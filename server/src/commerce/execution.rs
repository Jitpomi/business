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
            self.write(format!("{} select $t;distinct;put $fact isa commerce_plugin,has cx_id {key};(merchant:$t,plugin:$fact) isa commerce_plugin_owner;update $fact has cx_plugin {},has cx_enabled {hosted},has cx_refunds {refunds},has cx_verified {at};fetch {{'id':$fact.cx_id}};",self.prefix(),q(id))).await?;
        }
        Ok(())
    }
    pub async fn commerce_payment_details(&self, order: &str) -> Result<serde_json::Value> {
        self.require_access().await?;
        let rows=self.read(format!("{} $o isa commerce_order,has cx_id {}; (customer:$c,order:$o) isa commerce_order_owner;let $sub,$discount,$tax,$total=cx_totals($o);fetch {{'id':$o.cx_id,'total':$total,'invoices':[match (order:$o,plan:$p) isa commerce_execution;(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key,'amount':$i.bill_amount,'status':$i.bill_status,'url':$i.bill_url,'sequence':$i.bill_sequence}};]}};",self.customer_match(&self.commerce_customer(order).await?)?,q(order))).await?;
        rows.into_iter()
            .next()
            .ok_or_else(|| dog_core::DogError::not_found("Order unavailable").into_anyhow())
    }
    async fn commerce_customer(&self, order: &str) -> Result<String> {
        let rows=self.read(format!("{} $o isa commerce_order,has cx_id {}; (merchant:$t,customer:$c) isa bill_account;(customer:$c,order:$o) isa commerce_order_owner;fetch {{'id':$c.bill_key}};",self.prefix(),q(order))).await?;
        let row = rows
            .first()
            .ok_or_else(|| dog_core::DogError::not_found("Order unavailable").into_anyhow())?;
        Ok(string(row, "id")?.into())
    }
}
