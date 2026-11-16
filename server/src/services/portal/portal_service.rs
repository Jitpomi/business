//! Remaining invitation and consent commands, pending native-policy migration.
use super::{portal_schema as schema, portal_shared};
use crate::{
    billing::{billing_schema::quoted, engine::Engine},
    services::BusinessParams,
    typedb::TypeDBState,
};
use anyhow::{ensure, Result};
use dog_core::{DogError, DogService, ServiceCapabilities, TenantContext};
use serde_json::{json, Value};
use std::sync::Arc;
pub struct PortalService {
    access: crate::billing::access::BillingAccess,
}
impl PortalService {
    pub fn new(state: Arc<TypeDBState>, registry: Arc<crate::finance::TenantRegistry>) -> Self {
        Self {
            access: crate::billing::access::BillingAccess { state, registry },
        }
    }
}
fn now() -> String {
    chrono::Utc::now()
        .naive_utc()
        .format("%Y-%m-%dT%H:%M:%S%.6f")
        .to_string()
}
#[async_trait::async_trait]
impl DogService<Value, BusinessParams> for PortalService {
    fn capabilities(&self) -> ServiceCapabilities {
        portal_shared::capabilities()
    }
    async fn create(
        &self,
        ctx: &TenantContext,
        data: Value,
        params: BusinessParams,
    ) -> Result<Value> {
        let actor = schema::actor(ctx, &params)?;
        let e = if matches!(
            data["action"].as_str(),
            Some("invite" | "renew_invite" | "revoke_invite")
        ) {
            self.access.engine(&ctx.tenant_id.0).await?
        } else {
            self.access.engine_for(ctx, &actor, &data).await?
        };
        let action = schema::field(&data, "action")?;
        if matches!(action, "invite" | "renew_invite" | "revoke_invite") {
            ensure!(ctx.tenant_id.0 == e.merchant, "Merchant account required");
            let scope=format!("$viewer isa person,has identity_id {};let $owner=role_at_least($viewer,$t,\"owner\",{});$owner==true;",quoted(&actor),now());
            return e.invitation(&scope, &data).await;
        }
        if action == "reconcile_invoice" {
            let id = crate::billing::billing_schema::id(schema::field(&data, "invoice_id")?)?;
            let rows = e
                .read(format!(
                    "{} fetch {{'customer':$c.bill_key}};",
                    e.invoice_match(&id)?
                ))
                .await?;
            let row = rows
                .first()
                .ok_or_else(|| DogError::not_found("Invoice not found").into_anyhow())?;
            self.access
                .allowed(&e, ctx, &actor, schema::field(row, "customer")?, "seller")
                .await?;
            e.reconcile(&id).await?;
            return Ok(json!({"verified":true}));
        }

        let id = crate::billing::billing_schema::id(schema::field(&data, "customer_id")?)?;
        let permission = self.access.allowed(&e, ctx, &actor, &id, "buyer").await?;
        let customer = e.customer(&id).await?;
        let plugin_id = e.method_plugin(&customer)?;
        let plugin = e.runtime.financial(&plugin_id)?;
        plugin.ready()?;
        ensure!(
            plugin.capabilities().saved_methods,
            "Saved methods unsupported by this financial plugin"
        );
        let external = Engine::binding(&customer, &plugin_id)?;
        match action {
            "confirm_method" => {
                let method = plugin
                    .confirm_method(&external, &id, schema::field(&data, "session_id")?)
                    .await?;
                self.access.payment(&e, &customer, &method).await?;
                if customer["payment_method"] == "" {
                    e.write(format!(r#"{} {permission} $c has bill_payment_method "",has bill_revision {};select $t,$c;distinct;update $c has bill_payment_method {},has bill_method_plugin {},has bill_revision {},has bill_consent "",has bill_choice "manual";fetch {{"id":$c.bill_key}};"#,e.customer_match(&id)?,quoted(schema::field(&customer,"revision")?),quoted(&method),quoted(&plugin_id),quoted(&uuid::Uuid::new_v4().to_string()))).await?;
                }
                return Ok(json!({"setup_purpose":"save_only"}));
            }
            "authorize_saved" | "set_default" | "remove_method" => {}
            _ => return Err(DogError::bad_request("Action unavailable").into_anyhow()),
        }
        let revision = schema::field(&data, "expected_revision")?;
        ensure!(
            customer["revision"] == revision,
            "Payment settings changed. Refresh and review them again."
        );
        let next = uuid::Uuid::new_v4().to_string();
        if action == "authorize_saved" {
            ensure!(
                plugin.capabilities().automatic_collection,
                "Automatic collection unsupported by this financial plugin"
            );
            ensure!(
                customer["policy"] != "manual",
                "Automatic payments are not available for this account"
            );
            let method = schema::field(&customer, "payment_method")?;
            self.access.payment(&e, &customer, method).await?;
            let removed = e
                .read(format!(
                    "{} $c has bill_removed_method {};fetch {{'id':$c.bill_key}};",
                    e.customer_match(&id)?,
                    quoted(&format!("{plugin_id}:{method}"))
                ))
                .await?;
            ensure!(removed.is_empty(), "Payment method is being removed");
            let timestamp = chrono::Utc::now().timestamp();
            e.write(format!(r#"{} {permission} $c has bill_revision {}; (customer:$c,plan:$p) isa bill_plan_owner;$p has bill_status "active";select $t,$c,$viewer;distinct;update $c has bill_choice "automatic",has bill_consent {},has bill_revision {};insert $e isa bill_consent_evidence,has bill_key {},has bill_revision {},has bill_updated {timestamp};(customer:$c,actor:$viewer,evidence:$e) isa bill_consent_owner;fetch {{"id":$c.bill_key}};"#,e.customer_match(&id)?,quoted(revision),quoted(&next),quoted(&next),quoted(&next),quoted(&next))).await?;
            return Ok(json!({"authorized":true}));
        }
        let method = schema::field(&data, "method_id")?;
        self.access.payment(&e, &customer, method).await?;
        if action == "set_default" {
            let removed = e
                .read(format!(
                    "{} $c has bill_removed_method {};fetch {{'id':$c.bill_key}};",
                    e.customer_match(&id)?,
                    quoted(&format!("{plugin_id}:{method}"))
                ))
                .await?;
            ensure!(removed.is_empty(), "Payment method is being removed");
            e.write(format!(r#"{} {permission} $c has bill_revision {};select $t,$c;distinct;update $c has bill_payment_method {},has bill_method_plugin {},has bill_choice "manual",has bill_consent "",has bill_revision {};fetch {{"id":$c.bill_key}};"#,e.customer_match(&id)?,quoted(revision),quoted(method),quoted(&plugin_id),quoted(&next))).await?;
        } else {
            let default = customer["payment_method"] == method;
            let update = if default {
                format!(
                    r#"update $c has bill_payment_method "",has bill_choice "manual",has bill_consent "",has bill_revision {};"#,
                    quoted(&next)
                )
            } else {
                format!("update $c has bill_revision {};", quoted(&next))
            };
            e.write(format!(r#"{} {permission} $c has bill_revision {};select $t,$c;distinct;{update} put $c has bill_removed_method {};fetch {{"id":$c.bill_key}};"#,e.customer_match(&id)?,quoted(revision),quoted(&format!("{plugin_id}:{method}")))).await?;
            plugin
                .detach_method(method, &format!("detach-{id}-{plugin_id}-{method}"))
                .await?;
        }
        Ok(json!({"updated":true}))
    }
}
