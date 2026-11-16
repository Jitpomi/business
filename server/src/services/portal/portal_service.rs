//! Authenticated account discovery and provider-owned payment operations.
//! Ordinary billing CRUD remains in the schema-policy data service.
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
    state: Arc<TypeDBState>,
    registry: Arc<crate::finance::TenantRegistry>,
}
impl PortalService {
    pub fn new(state: Arc<TypeDBState>, registry: Arc<crate::finance::TenantRegistry>) -> Self {
        Self { state, registry }
    }
    async fn engine(&self, tenant: &str) -> Result<Engine> {
        let runtime = self.registry.resolve(tenant).await?;
        Engine::new(self.state.clone(), tenant, runtime)
    }
    async fn engine_for(&self, ctx: &TenantContext, actor: &str, data: &Value) -> Result<Engine> {
        if matches!(
            data["action"].as_str(),
            Some("invite" | "renew_invite" | "revoke_invite")
        ) {
            return self.engine(&ctx.tenant_id.0).await;
        }
        let selector = if let Some(id) = data["invoice_id"].as_str() {
            format!("(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;$i has bill_key {};",quoted(&crate::billing::billing_schema::id(id)?))
        } else {
            format!(
                "$c has bill_key {};",
                quoted(&crate::billing::billing_schema::id(schema::field(
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
        self.engine(schema::field(&rows[0], "merchant")?).await
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
    async fn allowed(
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
    async fn payment(
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
#[async_trait::async_trait]
impl DogService<Value, BusinessParams> for PortalService {
    fn capabilities(&self) -> ServiceCapabilities {
        portal_shared::capabilities()
    }
    async fn find(&self, ctx: &TenantContext, params: BusinessParams) -> Result<Vec<Value>> {
        let actor = schema::actor(ctx, &params)?;
        let e = self.engine(&ctx.tenant_id.0).await?;
        let query = format!(
            r#"match $u isa person,has identity_id {}; $t isa tenant,has tenant_id {};let $active=active_member($u,$t,{});$active==true;let $owner=role_at_least($u,$t,"owner",{});fetch {{"email":$u.email,"tenant_id":$t.tenant_id,"owner":$owner,"merchant_id":{},"live":{},"companies":[match $company isa tenant;let $member=active_member($u,$company,{});$member==true;fetch {{"id":$company.tenant_id,"domain":$company.name}};]}};"#,
            quoted(&actor),
            quoted(&ctx.tenant_id.0),
            now(),
            now(),
            quoted(&e.merchant),
            !e.runtime.sandbox,
            now()
        );
        e.read(query).await
    }
    async fn create(
        &self,
        ctx: &TenantContext,
        data: Value,
        params: BusinessParams,
    ) -> Result<Value> {
        let actor = schema::actor(ctx, &params)?;
        let e = self.engine_for(ctx, &actor, &data).await?;
        let action = schema::field(&data, "action")?;
        if matches!(action, "invite" | "renew_invite" | "revoke_invite") {
            ensure!(ctx.tenant_id.0 == e.merchant, "Merchant account required");
            let scope=format!("$viewer isa person,has identity_id {};let $owner=role_at_least($viewer,$t,\"owner\",{});$owner==true;",quoted(&actor),now());
            return e.invitation(&scope, &data).await;
        }
        if action == "history" || action == "reconcile_invoice" {
            let id = crate::billing::billing_schema::id(schema::field(&data, "invoice_id")?)?;
            let rows=e.read(format!("{} fetch {{'customer':$c.bill_key,'status':$i.bill_status,'amount':$i.bill_amount,'provider':$i.bill_provider,'updated':$i.bill_updated}};",e.invoice_match(&id)?)).await?;
            let row = rows
                .first()
                .ok_or_else(|| DogError::not_found("Invoice not found").into_anyhow())?;
            self.allowed(
                &e,
                ctx,
                &actor,
                schema::field(row, "customer")?,
                if action == "history" {
                    "reader"
                } else {
                    "seller"
                },
            )
            .await?;
            if action == "reconcile_invoice" {
                e.reconcile(&id).await?;
                return Ok(json!({"verified":true}));
            }
            let binding = e
                .read(format!(
                    "{} fetch {{'external':$i.bill_external_id}};",
                    e.invoice_match(&id)?
                ))
                .await?;
            ensure!(binding.len() == 1, "Invoice binding unavailable");
            let external = schema::field(&binding[0], "external")?;
            let plugin_id = schema::field(row, "provider")?;
            let plugin = e.runtime.financial(plugin_id)?;
            if !plugin.capabilities().payment_history {
                return Ok(
                    json!({"supported":false,"message":"Detailed payment history is not supported by this financial plugin."}),
                );
            }
            let invoice = plugin.observe_invoice(external).await?;
            e.validate_observation(&id, plugin_id, &invoice).await?;
            return plugin.history(external).await;
        }

        let id = crate::billing::billing_schema::id(schema::field(&data, "customer_id")?)?;
        let permission = self.allowed(&e, ctx, &actor, &id, "buyer").await?;
        let customer = e.customer(&id).await?;
        let bindings: std::collections::HashMap<String, String> =
            serde_json::from_str(schema::field(&customer, "bindings")?)?;
        if action == "payment_methods" && bindings.values().all(String::is_empty) {
            return Ok(json!({"methods":[]}));
        }
        let plugin_id = e.method_plugin(&customer)?;
        let plugin = e.runtime.financial(&plugin_id)?;
        plugin.ready()?;
        ensure!(
            plugin.capabilities().saved_methods,
            "Saved methods unsupported by this financial plugin"
        );
        let external = Engine::binding(&customer, &plugin_id)?;
        match action {
            "payment_methods" => {
                if external.is_empty() {
                    return Ok(json!({"methods":[]}));
                }
                let mut methods = vec![];
                for method in plugin.methods(&external).await? {
                    ensure!(
                        method.customer == external,
                        "Payment method customer mismatch"
                    );
                    let pending = e
                        .read(format!(
                            "{} $c has bill_removed_method {};fetch {{'id':$c.bill_key}};",
                            e.customer_match(&id)?,
                            quoted(&format!("{plugin_id}:{}", method.id))
                        ))
                        .await?;
                    let default = customer["payment_method"] == method.id
                        && customer["method_plugin"] == plugin_id;
                    let mut value = serde_json::to_value(method)?;
                    value["plugin_id"] = json!(plugin_id);
                    value["is_default"] = json!(default);
                    value["removal_pending"] = json!(!pending.is_empty());
                    value["automatic_enabled"] = json!(
                        pending.is_empty()
                            && default
                            && customer["choice"] == "automatic"
                            && customer["consent"] == customer["revision"]
                            && customer["policy"] != "manual"
                    );
                    methods.push(value);
                }
                return Ok(json!({"methods":methods}));
            }
            "setup_card" | "setup_method" => {
                ensure!(data["save_only"] == true, "Save a payment method first");
                let external = e.ensure_customer(&customer, &plugin_id).await?;
                let url = plugin
                    .setup_method(
                        &external,
                        &id,
                        data["bank_account"] == true,
                        "https://jitpomi.com/account/",
                        &format!("save-method-{}", uuid::Uuid::new_v4()),
                    )
                    .await?;
                return Ok(json!({"url":url}));
            }
            "confirm_card" | "confirm_method" => {
                let method = plugin
                    .confirm_method(&external, &id, schema::field(&data, "session_id")?)
                    .await?;
                self.payment(&e, &customer, &method).await?;
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
            self.payment(&e, &customer, method).await?;
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
        self.payment(&e, &customer, method).await?;
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
