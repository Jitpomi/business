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
use dog_core::{DogError, DogService, ServiceCapabilities, ServiceMethodKind, TenantContext};
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
        let now = crate::clock::native_utc();
        // The native policy checks current membership. Configuration is resolved
        // only after this succeeds; no credentials are returned to the caller.
        let rows=crate::storage::read(&self.access.state,&format!("match let $email,$tenant,$owner=request_account({},{},{}); fetch {{'tenant':$tenant}};",q(&ctx.tenant_id.0),q(&identity),now)).await?;
        ensure!(
            rows.len() == 1,
            DogError::forbidden("Financial configuration unavailable").into_anyhow()
        );
        let configuration = self.access.registry.configuration(&ctx.tenant_id.0).await?;
        Ok(vec![serde_json::to_value(configuration)?])
    }
    async fn create(
        &self,
        ctx: &TenantContext,
        data: Value,
        params: BusinessParams,
    ) -> Result<Value> {
        let actor = actor(ctx, &params)?;
        let action = super::financial_schema::action(&data)?;
        if action == "checkout_configuration" {
            let e = self.access.owner_engine(ctx, &actor).await?;
            return Ok(json!({"plugins":e.attest_checkout_plugins().await?}));
        }
        let e = self.access.engine_for(ctx, &actor, &data).await?;
        if action == "checkout_options" {
            return Ok(json!({"plugins":e.attest_checkout_plugins().await?}));
        }
        if action == "payment_form" {
            let id = crate::billing::billing_schema::id(field(&data, "invoice_id")?)?;
            let origin = data
                .get("embed_origin")
                .map(|v| {
                    v.as_str().ok_or_else(|| {
                        DogError::bad_request("Invalid embedding origin").into_anyhow()
                    })
                })
                .transpose()?
                .unwrap_or("");
            if !origin.is_empty() {
                let parsed = reqwest::Url::parse(origin)
                    .map_err(|_| DogError::bad_request("Invalid embedding origin").into_anyhow())?;
                ensure!(
                    parsed.scheme() == "https" && parsed.origin().ascii_serialization() == origin,
                    DogError::bad_request("Use an exact HTTPS origin").into_anyhow()
                );
            }
            let selected = e
                .read(format!(
                    "{} let $state_ready=cx_inline_state($i);$state_ready==true;fetch {{'plugin':$i.bill_provider,'merchant_name':$t.name}};",
                    e.invoice_match(&id)?
                ))
                .await?;
            let selected = selected
                .first()
                .filter(|_| selected.len() == 1)
                .ok_or_else(|| {
                    DogError::conflict("This invoice is not available for manual payment")
                        .into_anyhow()
                })?;
            let plugin_id = field(selected, "plugin")?;
            let plugin = e.runtime.financial(plugin_id)?;
            ensure!(plugin.capabilities().inline_payment,DogError::bad_request("Inline payments are unavailable for this connection; use its hosted payment link").into_anyhow());
            // The existing attestation producer disables checkout capabilities
            // outside sandbox and records them as fresh native policy facts.
            e.attest_checkout_plugins().await?;
            let eligibility = |payable: Option<i64>| -> Result<String> {
                let amount=payable.map(|value| format!("let $amount_allowed=cx_inline_amount_valid($i,{value});$amount_allowed==true;")).unwrap_or_default();
                Ok(format!("{} let $allowed=cx_inline_invoice({},{},{},$i,{});$allowed==true;{amount}fetch {{'external':$i.bill_external_id,'plugin':$i.bill_provider}};",e.invoice_match(&id)?,quoted(&ctx.tenant_id.0),quoted(&actor),crate::clock::native_utc(),quoted(origin)))
            };
            let rows = e.read(eligibility(None)?).await?;
            let row = rows.first().filter(|_| rows.len() == 1).ok_or_else(|| {
                DogError::forbidden("This invoice or embedding site is unavailable for payment")
                    .into_anyhow()
            })?;
            ensure!(
                row["plugin"] == plugin_id,
                DogError::conflict("Invoice connection changed; refresh it").into_anyhow()
            );
            plugin.ready()?;
            let presentation = plugin.payment_form(field(row, "external")?).await?;
            e.validate_observation(&id, plugin_id, &presentation.invoice)
                .await
                .map_err(|error| {
                    DogError::bad_gateway("Provider payment form does not match this invoice")
                        .with_source(error)
                        .into_anyhow()
                })?;
            ensure!(
                presentation.invoice.state == crate::finance::InvoiceState::Open,
                DogError::conflict("Refresh this invoice before paying").into_anyhow()
            );
            // Recheck current native permission, origin approval and invoice state
            // after provider I/O; never return a form under an expired decision.
            ensure!(
                e.read(eligibility(Some(presentation.payable_minor))?)
                    .await?
                    == rows,
                DogError::forbidden("Payment access changed; refresh this invoice").into_anyhow()
            );
            return Ok(
                json!({"invoice_id":id,"merchant_id":e.merchant,"merchant_name":selected["merchant_name"],"amount":presentation.payable_minor,"invoice_amount":presentation.invoice.amount_minor,"currency":presentation.invoice.currency,"embed_origin":origin,"form":presentation.form}),
            );
        }
        if action == "history" {
            let id = crate::billing::billing_schema::id(field(&data, "invoice_id")?)?;
            let rows=e.read(format!("{} fetch {{'customer':$c.bill_key,'status':$i.bill_status,'amount':$i.bill_amount,'provider':$i.bill_provider,'updated':$i.bill_updated}};",e.invoice_match(&id)?)).await?;
            let row = rows
                .first()
                .ok_or_else(|| DogError::not_found("Invoice not found").into_anyhow())?;
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
        let e = e.with_permission(BillingAccess::permission(ctx, &actor, &id, "buyer")?);
        e.require_access().await?;
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
                let mut return_url = reqwest::Url::parse(if data["checkout"] == true {
                    "https://jitpomi.com/checkout/"
                } else {
                    "https://jitpomi.com/account/"
                })?;
                if data["checkout"] == true {
                    return_url
                        .query_pairs_mut()
                        .append_pair("merchant", &e.merchant)
                        .append_pair("plugin", &plugin_id);
                }
                let url = plugin
                    .setup_method(
                        &external,
                        &id,
                        data["bank_account"] == true,
                        return_url.as_str(),
                        &format!("save-method-{}", uuid::Uuid::new_v4()),
                    )
                    .await?;
                Ok(json!({"url":url}))
            }
            _ => Err(DogError::bad_request("Unsupported financial plugin operation").into_anyhow()),
        }
    }
}
