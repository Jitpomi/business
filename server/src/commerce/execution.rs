//! Native orders become ordinary billing plans once; providers are resolved by their merchant.
use crate::billing::{
    billing_schema::quoted as q,
    engine::{string, Engine},
};
use anyhow::Result;
impl Engine {
    pub async fn activate_commerce_orders(&self) -> Result<usize> {
        if !self.runtime.sandbox {
            return Ok(0);
        }
        let at = crate::clock::native_utc();
        let rows=self.read(format!("{} $o isa commerce_order,has cx_created $created;let $allowed=cx_can_activate($t,$o,{at});$allowed==true;select $o,$created;sort $created;limit 10;fetch {{'id':$o.cx_id}};",self.prefix())).await?;
        let mut count = 0;
        for row in rows {
            let id = q(string(&row, "id")?);
            let plan = q(&uuid::Uuid::new_v4().to_string());
            let now = crate::clock::unix_seconds();
            let at = crate::clock::native_utc();
            let result = self.write(format!("{} $o isa commerce_order,has cx_id {id};let $allowed=cx_can_activate($t,$o,{at});$allowed==true;(order:$o,customer:$c) isa commerce_order_owner;let $sub,$discount,$tax,$total=cx_totals($o);$o has cx_interval $period;select $t,$o,$c,$total,$period;distinct;insert $p isa bill_plan,has bill_key {plan},has name 'Checkout order',has bill_amount == $total,has bill_due_days 0,has bill_anchor {now},has bill_next {now},has bill_sequence 0,has bill_interval == $period,has bill_status 'active';(customer:$c,plan:$p) isa bill_plan_owner;(order:$o,plan:$p) isa commerce_execution;fetch {{'id':$p.bill_key}};",self.prefix())).await;
            if let Err(error) = result {
                // A concurrent worker or lost acknowledgement may have committed the plan.
                // Resolve only the immutable order journal, without retrying the insert.
                let recorded = self.read(format!("{} $o isa commerce_order,has cx_id {id};(order:$o,plan:$p) isa commerce_execution;fetch {{'id':$p.bill_key}};", self.prefix())).await?;
                if recorded.len() != 1 {
                    return Err(error);
                }
                continue;
            }
            count += 1;
        }
        Ok(count)
    }
    pub async fn attest_checkout_plugins(&self) -> Result<Vec<serde_json::Value>> {
        self.require_access().await?;
        anyhow::ensure!(
            self.runtime.sandbox,
            dog_core::DogError::forbidden("Real checkout payments are disabled").into_anyhow()
        );
        let plugins = self.runtime.checkout_plugins();
        self.refresh_commerce_plugins().await?;
        Ok(plugins)
    }
    pub async fn refresh_commerce_plugins(&self) -> Result<()> {
        // Checkout remains sandbox-only, including scheduled execution.
        let plugins = self.runtime.checkout_plugins();
        for plugin in &plugins {
            let id = string(plugin, "id")?;
            let key = q(&format!("{}:{id}", self.merchant));
            let at = crate::clock::native_utc();
            let ready = self.runtime.financial(id)?.ready().is_ok();
            let hosted =
                self.runtime.sandbox && ready && plugin["capabilities"]["hosted_checkout"] == true;
            let refunds =
                self.runtime.sandbox && ready && plugin["capabilities"]["refunds"] == true;
            let currencies = self.runtime.financial(id)?.currencies();
            anyhow::ensure!(
                !currencies.is_empty(),
                "Plugin must declare supported currencies"
            );
            let owns = currencies
                .iter()
                .map(|v| format!(",has cx_supported_currency {}", q(v)))
                .collect::<String>();
            // The entire attested set and freshness timestamp change in one fenced commit.
            self.write(format!("{} select $t;distinct;put $fact isa commerce_plugin,has cx_id {key};(merchant:$t,plugin:$fact) isa commerce_plugin_owner;match try {{$fact has cx_supported_currency $old;}};delete try {{has $old of $fact;}};select $t,$fact;distinct;update $fact has cx_plugin {},has cx_enabled {hosted},has cx_refunds {refunds},has cx_verified {at};insert $fact {} ;fetch {{'id':$fact.cx_id}};",self.prefix(),q(id),owns.trim_start_matches(','))).await?;
        }
        Ok(())
    }
}
