//! Persist externally verified facts; native policies own all customer choices.
use super::{billing_schema::quoted as q, engine::Engine};
use anyhow::{ensure, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
impl Engine {
    pub async fn attest_method(
        &self,
        customer: &str,
        plugin: &str,
        method: &crate::finance::SavedMethod,
    ) -> Result<()> {
        crate::finance::opaque_id(&method.id)?;
        let key = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(customer, plugin, &method.id))?)
        );
        let automatic = self
            .runtime
            .financial(plugin)?
            .capabilities()
            .automatic_collection;
        let at = chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6f")
            .to_string();
        let existing=self.read(format!("{} $m isa bill_saved_method,has bill_key {}; (customer:$c,method:$m) isa bill_method_owner; fetch {{'id':$m.method_ref,'plugin':$m.method_plugin}};",self.customer_match(customer)?,q(&key))).await?;
        if let Some(old) = existing.first() {
            ensure!(
                existing.len() == 1 && old["id"] == method.id && old["plugin"] == plugin,
                "Method attestation binding mismatch"
            );
            self.write(format!("{} $m isa bill_saved_method,has bill_key {}; (customer:$c,method:$m) isa bill_method_owner;select $t,$m;distinct;update $m has method_verified_at {at},has method_automatic {automatic};fetch {{'id':$m.bill_key}};",self.customer_match(customer)?,q(&key))).await?;
        } else {
            let pending = !self
                .read(format!(
                    "{} $c has bill_removed_method {};fetch {{'id':$c.bill_key}};",
                    self.customer_match(customer)?,
                    q(&format!("{plugin}:{}", method.id))
                ))
                .await?
                .is_empty();
            self.write(format!("{} not {{$m isa bill_saved_method,has bill_key {};}};select $t,$c;distinct;insert $m isa bill_saved_method,has bill_key {},has method_ref {},has method_plugin {},has method_automatic {automatic},has method_pending {pending},has method_active true,has method_verified_at {at};(customer:$c,method:$m) isa bill_method_owner;fetch {{'id':$m.bill_key}};",self.customer_match(customer)?,q(&key),q(&key),q(&method.id),q(plugin))).await?;
        }
        Ok(())
    }
    pub async fn method_removal(
        &self,
        customer: &str,
        plugin: &str,
        method: &str,
    ) -> Result<Vec<Value>> {
        self.read(format!("{} $m isa bill_saved_method,has method_ref {},has method_plugin {},has method_pending true;(customer:$c,method:$m) isa bill_method_owner;not {{$c has bill_payment_method {},has bill_method_plugin {};}};fetch {{'id':$m.bill_key}};",self.customer_match(customer)?,q(method),q(plugin),q(method),q(plugin))).await
    }
}
