//! Shared access to tenant financial runtimes; permissions are native TypeQL.
use super::{billing_schema::quoted, engine::Engine};
use crate::{finance::TenantRegistry, typedb::TypeDBState};
use anyhow::Result;
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
#[derive(Clone)]
pub(crate) struct Permission {
    tenant: String,
    actor: String,
    customer: String,
    kind: &'static str,
}
impl Permission {
    pub(crate) fn clause(&self) -> String {
        format!(
            r#"$financial_actor isa person,has identity_id {};$financial_tenant isa tenant,has tenant_id {};$c has bill_key {};let $financial_scope_allowed=bill_{}($financial_actor,$financial_tenant,$c,{});$financial_scope_allowed==true;"#,
            quoted(&self.actor),
            quoted(&self.tenant),
            quoted(&self.customer),
            self.kind,
            now()
        )
    }
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
        let rows=e.read(format!("match $viewer isa person,has identity_id {};$scope isa tenant,has tenant_id {};$merchant isa tenant;$c isa bill_customer;(merchant:$merchant,customer:$c) isa bill_account;{selector}let $allowed=bill_reader($viewer,$scope,$c,{});$allowed==true;select $merchant,$c;distinct;fetch {{'merchant':$merchant.tenant_id,'customer':$c.bill_key}};",quoted(actor),quoted(&ctx.tenant_id.0),now())).await?;
        if rows.len() != 1 {
            return Err(DogError::forbidden("Billing account unavailable").into_anyhow());
        }
        let customer = field(&rows[0], "customer")?;
        let engine = self.engine(field(&rows[0], "merchant")?).await?;
        Ok(engine.with_permission(Self::permission(ctx, actor, customer, "reader")?))
    }
    pub(crate) fn permission(
        ctx: &TenantContext,
        actor: &str,
        customer: &str,
        kind: &str,
    ) -> Result<Permission> {
        let kind = match kind {
            "buyer" => "buyer",
            "reader" => "reader",
            "seller" => "seller",
            _ => anyhow::bail!("Invalid permission"),
        };
        Ok(Permission {
            tenant: ctx.tenant_id.0.clone(),
            actor: actor.into(),
            customer: crate::billing::billing_schema::id(customer)?,
            kind,
        })
    }
}
fn now() -> String {
    chrono::Utc::now()
        .naive_utc()
        .format("%Y-%m-%dT%H:%M:%S%.6f")
        .to_string()
}
