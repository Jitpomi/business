// Tenant financial-plugin metadata and external operations. Account data belongs to /data.
use crate::{
    billing::{
        access::{field, BillingAccess},
        billing_schema::quoted,
        engine::Engine,
    },
    finance::TenantRegistry,
    services::{actor, BusinessParams},
    typedb::TypeDBState,
};
use anyhow::{ensure, Result};
use dog_core::{
    DogAppBuilder, DogError, DogService, ServiceCapabilities, ServiceMethodKind, TenantContext,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct FinancialService {
    access: BillingAccess,
}
impl FinancialService {
    pub fn new(state: Arc<TypeDBState>, registry: Arc<TenantRegistry>) -> Self {
        Self {
            access: BillingAccess { state, registry },
        }
    }
}
#[async_trait::async_trait]
impl DogService<Value, BusinessParams> for FinancialService {
    fn capabilities(&self) -> ServiceCapabilities {
        ServiceCapabilities::from_methods(vec![ServiceMethodKind::Find, ServiceMethodKind::Create])
    }
    async fn find(&self, ctx: &TenantContext, params: BusinessParams) -> Result<Vec<Value>> {
        let identity = actor(ctx, &params)?;
        let q = |s: &str| serde_json::to_string(s).unwrap();
        let now = chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6f")
            .to_string();
        // The native policy checks current membership. Configuration is resolved
        // only after this succeeds; no credentials are returned to the caller.
        let rows=crate::storage::read(&self.access.state,&format!("match let $email,$tenant,$owner=request_account({},{},{}); fetch {{'tenant':$tenant}};",q(&ctx.tenant_id.0),q(&identity),now)).await?;
        ensure!(
            rows.len() == 1,
            DogError::forbidden("Financial configuration unavailable").into_anyhow()
        );
        let runtime = self.access.registry.resolve(&ctx.tenant_id.0).await?;
        Ok(vec![
            json!({"sandbox":runtime.sandbox,"default_financial":runtime.default_financial}),
        ])
    }
    async fn create(
        &self,
        ctx: &TenantContext,
        data: Value,
        params: BusinessParams,
    ) -> Result<Value> {
        let actor = actor(ctx, &params)?;
        let action = field(&data, "action")?;
        ensure!(
            matches!(
                action,
                "history" | "payment_methods" | "setup_method" | "confirm_method" | "detach_method"
            ),
            DogError::bad_request("Unsupported financial plugin operation").into_anyhow()
        );
        let e = self.access.engine_for(ctx, &actor, &data).await?;
        if action == "history" {
            let id = crate::billing::billing_schema::id(field(&data, "invoice_id")?)?;
            let rows=e.read(format!("{} fetch {{'customer':$c.bill_key,'status':$i.bill_status,'amount':$i.bill_amount,'provider':$i.bill_provider,'updated':$i.bill_updated}};",e.invoice_match(&id)?)).await?;
            let row = rows
                .first()
                .ok_or_else(|| DogError::not_found("Invoice not found").into_anyhow())?;
            self.access
                .allowed(&e, ctx, &actor, field(row, "customer")?, "reader")
                .await?;
            let binding = e
                .read(format!(
                    "{} fetch {{'external':$i.bill_external_id}};",
                    e.invoice_match(&id)?
                ))
                .await?;
            ensure!(binding.len() == 1, "Invoice binding unavailable");
            let external = field(&binding[0], "external")?;
            let plugin_id = field(row, "provider")?;
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

        let id = crate::billing::billing_schema::id(field(&data, "customer_id")?)?;
        self.access.allowed(&e, ctx, &actor, &id, "buyer").await?;
        let customer = e.customer(&id).await?;
        let bindings: std::collections::HashMap<String, String> =
            serde_json::from_str(field(&customer, "bindings")?)?;
        if action == "payment_methods" && bindings.values().all(String::is_empty) {
            return Ok(json!({"methods":[]}));
        }
        let plugin_id = data["plugin_id"]
            .as_str()
            .map(str::to_owned)
            .map(Ok)
            .unwrap_or_else(|| e.method_plugin(&customer))?;
        let plugin = e.runtime.financial(&plugin_id)?;
        plugin.ready()?;
        ensure!(
            plugin.capabilities().saved_methods,
            DogError::bad_request("Saved methods unsupported by this financial plugin")
                .into_anyhow()
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
                    e.attest_method(&id, &plugin_id, &method).await?;
                    let pending = e.method_removal(&id, &plugin_id, &method.id).await?;
                    let default = customer["payment_method"] == method.id
                        && customer["method_plugin"] == plugin_id;
                    let mut value = serde_json::to_value(method)?;
                    value["plugin_id"] = json!(plugin_id);
                    value["is_default"] = json!(default);
                    value["removal_pending"] = json!(!pending.is_empty());
                    value["automatic_enabled"] =
                        json!(default && customer["choice"] == "automatic");
                    methods.push(value);
                }
                Ok(json!({"methods":methods}))
            }
            "confirm_method" => {
                let method = plugin
                    .confirm_method(&external, &id, field(&data, "session_id")?)
                    .await?;
                let verified = plugin.method(&external, &method).await?;
                ensure!(
                    verified.customer == external && verified.id == method,
                    "Saved method binding mismatch"
                );
                e.attest_method(&id, &plugin_id, &verified).await?;
                Ok(json!({"method_id":method,"plugin_id":plugin_id,"setup_purpose":"save_only"}))
            }
            "detach_method" => {
                let method = field(&data, "method_id")?;
                let pending = e.method_removal(&id, &plugin_id, method).await?;
                ensure!(
                    pending.len() == 1,
                    DogError::conflict("Record removal intent through /data first").into_anyhow()
                );
                plugin
                    .detach_method(
                        &external,
                        method,
                        &format!("detach-{id}-{plugin_id}-{method}"),
                    )
                    .await.map_err(|error|DogError::bad_gateway("Removal is pending. Refresh and retry when your payment provider is available.").with_source(error).into_anyhow())?;
                e.write(format!("{} $m isa bill_saved_method,has bill_key {}; (customer:$c,method:$m) isa bill_method_owner;select $t,$m;distinct;update $m has method_active false;fetch {{'id':$m.bill_key}};",e.customer_match(&id)?,quoted(field(&pending[0],"id")?))).await?;
                Ok(json!({"removed":true}))
            }
            "setup_method" => {
                ensure!(
                    data["save_only"] == true,
                    DogError::bad_request("Save a payment method first").into_anyhow()
                );
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
                Ok(json!({"url":url}))
            }
            _ => Err(DogError::bad_request("Unsupported financial plugin operation").into_anyhow()),
        }
    }
}
pub fn register_hooks(
    builder: &mut DogAppBuilder<Value, BusinessParams>,
    auth: Arc<dog_auth::AuthenticationService<BusinessParams>>,
) {
    builder.service_hooks("financial", |h| {
        h.before_all(Arc::new(
            dog_auth::hooks::AuthenticateHook::new(auth, vec!["jwt".into()]).strict(),
        ));
    });
}
