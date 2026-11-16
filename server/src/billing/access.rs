//! Shared access to tenant financial runtimes; permissions are native TypeQL.
use super::{billing_schema::quoted, engine::Engine};
use crate::{finance::TenantRegistry, typedb::TypeDBState};
use anyhow::{ensure, Result};
use dog_core::{DogError, TenantContext};
use serde_json::Value;
use std::sync::Arc;
pub(crate) struct BillingAccess {
    pub state: Arc<TypeDBState>,
    pub registry: Arc<TenantRegistry>,
}
pub(crate) fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .filter(|s| !s.is_empty() && s.len() < 512)
        .ok_or_else(|| DogError::bad_request("Required information is missing").into_anyhow())
}
impl BillingAccess {
    pub(crate) async fn engine(&self, tenant: &str) -> Result<Engine> {
        let runtime = self.registry.resolve(tenant).await?;
        Engine::new(self.state.clone(), tenant, runtime)
    }
    pub(crate) async fn engine_for(
        &self,
        ctx: &TenantContext,
        actor: &str,
        data: &Value,
    ) -> Result<Engine> {
        let selector = if let Some(id) = data["invoice_id"].as_str() {
            format!("(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;$i has bill_key {};",quoted(&crate::billing::billing_schema::id(id)?))
        } else {
            format!(
                "$c has bill_key {};",
                quoted(&crate::billing::billing_schema::id(field(
                    data,
                    "customer_id"
                )?)?)
            )
        };
        let e = self.engine(&ctx.tenant_id.0).await?;
        let rows=e.read(format!("match $viewer isa person,has identity_id {};$scope isa tenant,has tenant_id {};$merchant isa tenant;$c isa bill_customer;(merchant:$merchant,customer:$c) isa bill_account;{selector}let $allowed=bill_reader($viewer,$scope,$c,{});$allowed==true;select $merchant;distinct;fetch {{'merchant':$merchant.tenant_id}};",quoted(actor),quoted(&ctx.tenant_id.0),now())).await?;
        if rows.len() != 1 {
            return Err(DogError::forbidden("Billing account unavailable").into_anyhow());
        }
        self.engine(field(&rows[0], "merchant")?).await
    }
    fn permission(ctx: &TenantContext, actor: &str, customer: &str, kind: &str) -> Result<String> {
        ensure!(
            matches!(kind, "buyer" | "reader" | "seller"),
            "Invalid permission"
        );
        let customer = crate::billing::billing_schema::id(customer)?;
        Ok(format!(
            r#"$viewer isa person,has identity_id {};$scope isa tenant,has tenant_id {}; $c has bill_key "{customer}";let $allowed=bill_{kind}($viewer,$scope,$c,{});$allowed==true;"#,
            quoted(actor),
            quoted(&ctx.tenant_id.0),
            now()
        ))
    }
    pub(crate) async fn allowed(
        &self,
        e: &Engine,
        ctx: &TenantContext,
        actor: &str,
        customer: &str,
        kind: &str,
    ) -> Result<String> {
        let permission = Self::permission(ctx, actor, customer, kind)?;
        let rows = e
            .read(format!(
                "{} {permission} fetch {{'id':$c.bill_key}};",
                e.customer_match(customer)?
            ))
            .await?;
        if rows.len() != 1 {
            return Err(
                DogError::forbidden("You do not have access to this billing account").into_anyhow(),
            );
        }
        Ok(permission)
    }
    pub(crate) async fn payment(
        &self,
        e: &Engine,
        customer: &Value,
        method: &str,
    ) -> Result<crate::finance::SavedMethod> {
        let plugin_id = e.method_plugin(customer)?;
        let external = Engine::binding(customer, &plugin_id)?;
        let saved = e
            .runtime
            .financial(&plugin_id)?
            .method(&external, method)
            .await?;
        ensure!(
            saved.customer == external && saved.id == method,
            "Saved method binding mismatch"
        );
        Ok(saved)
    }
}
fn now() -> String {
    chrono::Utc::now()
        .naive_utc()
        .format("%Y-%m-%dT%H:%M:%S%.6f")
        .to_string()
}
