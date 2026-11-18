use super::builtin_transport::{self as transport, field, pairs, safe_id, Provider, Providers};
use crate::{billing::billing_schema::Mode, finance::*};
use anyhow::{ensure, Result};
use async_trait::async_trait;
use reqwest::Method;
use serde_json::{json, Value};
pub struct BuiltinFinancial {
    pub transport: Providers,
    pub provider: Provider,
    pub webhook_secret: Option<String>,
}
impl BuiltinFinancial {
    pub fn observation(&self, v: &Value) -> Result<InvoiceObservation> {
        let (customer, local, amount, currency) = match self.provider {
            Provider::Stripe => (
                field(v, "customer")?.to_owned(),
                field(&v["metadata"], "business_invoice")?.to_owned(),
                v["total"]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Invalid invoice total"))?,
                field(v, "currency")?.to_ascii_uppercase(),
            ),
            Provider::Mercury => (
                field(v, "customerId")?.to_owned(),
                field(v, "invoiceNumber")?
                    .strip_prefix("JIT-")
                    .ok_or_else(|| anyhow::anyhow!("Invoice binding missing"))?
                    .to_owned(),
                transport::decimal_cents(&v["amount"])?,
                field(v, "currencyCode")?.to_owned(),
            ),
        };
        let state = serde_json::from_value(json!(transport::status(self.provider, v)?))?;
        Ok(InvoiceObservation {
            external_id: field(v, "id")?.into(),
            customer,
            invoice_id: local,
            amount_minor: amount,
            currency,
            state,
            payment_url: transport::pay_url(self.provider, v, self.transport.live)?,
        })
    }
    fn request(&self, r: &InvoiceRequest) -> Result<transport::InvoiceRequest> {
        ensure!(
            self.currencies().contains(&r.currency.as_str()),
            "Selected plugin does not support this invoice currency"
        );
        Ok(transport::InvoiceRequest {
            id: r.id.clone(),
            currency: r.currency.clone(),
            customer: r.customer.clone(),
            provider: self.provider,
            mode: if r.automatic {
                Mode::Automatic
            } else {
                Mode::Manual
            },
            amount: r.amount_minor,
            description: r.description.clone(),
            issued: r.issued,
            due: r.due,
        })
    }
    fn saved(&self, p: &Value, customer: &str) -> Result<SavedMethod> {
        ensure!(
            p["customer"] == customer,
            "Payment method customer mismatch"
        );
        let kind = field(p, "type")?;
        ensure!(
            matches!(kind, "card" | "us_bank_account"),
            "Unsupported saved method"
        );
        let part = &p[kind];
        Ok(SavedMethod {
            id: field(p, "id")?.into(),
            customer: customer.into(),
            kind: kind.into(),
            brand: part["brand"].as_str().map(str::to_owned),
            bank_name: part["bank_name"].as_str().map(str::to_owned),
            last4: part["last4"].as_str().map(str::to_owned),
            exp_month: part["exp_month"].as_u64(),
            exp_year: part["exp_year"].as_u64(),
        })
    }
}
#[async_trait]
impl FinancialPlugin for BuiltinFinancial {
    fn currencies(&self) -> &[&str] {
        match self.provider {
            Provider::Stripe => &["USD", "EUR", "GBP", "CAD", "AUD", "JPY"],
            Provider::Mercury => &["USD"],
        }
    }
    fn method_currencies(&self, method: &SavedMethod) -> &[&str] {
        if self.provider == Provider::Stripe && method.kind == "card" {
            self.currencies()
        } else {
            &["USD"]
        }
    }
    async fn webhook(
        &self,
        headers: &std::collections::HashMap<String, String>,
        body: &[u8],
    ) -> std::result::Result<Option<InvoiceObservation>, WebhookError> {
        if self.provider != Provider::Stripe {
            return Err(WebhookError::Invalid);
        }
        super::signature::verify_signature(
            self.webhook_secret.as_deref().unwrap_or(""),
            headers
                .get("stripe-signature")
                .map(String::as_str)
                .unwrap_or(""),
            body,
            crate::clock::unix_seconds(),
        )
        .map_err(|_| WebhookError::Unauthorized)?;
        let event: Value = serde_json::from_slice(body).map_err(|_| WebhookError::Invalid)?;
        if event["account"].as_str() != self.transport.connected_account.as_deref()
            || event["livemode"].as_bool() != Some(self.transport.live)
        {
            return Err(WebhookError::Invalid);
        }
        if !event["type"].as_str().unwrap_or("").starts_with("invoice.")
            || event
                .pointer("/data/object/metadata/business_invoice")
                .and_then(Value::as_str)
                .is_none()
        {
            return Ok(None);
        }
        let external = event
            .pointer("/data/object/id")
            .and_then(Value::as_str)
            .ok_or(WebhookError::Invalid)?;
        self.observe_invoice(external)
            .await
            .map(Some)
            .map_err(|_| WebhookError::Unavailable)
    }
    fn brand(&self) -> Option<&'static str> {
        Some(match self.provider {
            Provider::Stripe => "stripe",
            Provider::Mercury => "mercury",
        })
    }
    fn capabilities(&self) -> Capabilities {
        let stripe = self.provider == Provider::Stripe;
        Capabilities {
            idempotent_customers: stripe,
            automatic_collection: stripe,
            saved_methods: stripe,
            payment_history: stripe,
            hosted_checkout: true,
            refunds: stripe,
        }
    }
    fn ready(&self) -> Result<()> {
        self.transport.ready(self.provider)
    }
    async fn create_customer(&self, key: &str, name: &str, email: &str) -> Result<String> {
        self.transport
            .customer(self.provider, key, name, email)
            .await
    }
    async fn create_invoice(&self, r: &InvoiceRequest) -> Result<String> {
        let v = self.transport.invoice(&self.request(r)?).await?;
        Ok(field(&v, "id")?.into())
    }
    async fn finalize_invoice(
        &self,
        r: &InvoiceRequest,
        external: &str,
    ) -> Result<InvoiceObservation> {
        if self.provider == Provider::Stripe {
            self.transport
                .finish_invoice(&self.request(r)?, external)
                .await?;
        }
        self.observe_invoice(external).await
    }
    async fn observe_invoice(&self, external: &str) -> Result<InvoiceObservation> {
        self.observation(&self.transport.observe(self.provider, external).await?)
    }
    async fn collect(&self, external: &str, key: &str, method: &str) -> Result<()> {
        ensure!(
            self.capabilities().automatic_collection,
            "Automatic collection unsupported"
        );
        self.transport.charge(external, key, method).await?;
        Ok(())
    }
    async fn methods(&self, customer: &str) -> Result<Vec<SavedMethod>> {
        ensure!(
            self.capabilities().saved_methods,
            "Saved methods unsupported"
        );
        safe_id(customer)?;
        let mut out = vec![];
        for kind in ["card", "us_bank_account"] {
            let mut after = String::new();
            for page in 0..10 {
                let mut query = pairs(&[("type", kind), ("limit", "100")]);
                if !after.is_empty() {
                    query.push(("starting_after".into(), after.clone()));
                }
                let list = self
                    .transport
                    .stripe(
                        Method::GET,
                        &format!("/customers/{customer}/payment_methods"),
                        "",
                        &query,
                    )
                    .await?;
                let data = list["data"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("Invalid payment method list"))?;
                for p in data {
                    out.push(self.saved(p, customer)?);
                }
                if list["has_more"] == false {
                    break;
                }
                ensure!(page < 9, "Payment method list too large");
                let next = field(
                    data.last()
                        .ok_or_else(|| anyhow::anyhow!("Incomplete payment method list"))?,
                    "id",
                )?;
                ensure!(next != after, "Payment method cursor stalled");
                after = next.into();
            }
        }
        Ok(out)
    }
    async fn method(&self, customer: &str, id: &str) -> Result<SavedMethod> {
        ensure!(
            self.capabilities().saved_methods,
            "Saved methods unsupported"
        );
        safe_id(id)?;
        self.saved(
            &self
                .transport
                .stripe(Method::GET, &format!("/payment_methods/{id}"), "", &[])
                .await?,
            customer,
        )
    }
    async fn setup_method(
        &self,
        customer: &str,
        local: &str,
        bank: bool,
        return_url: &str,
        key: &str,
    ) -> Result<String> {
        ensure!(
            self.capabilities().saved_methods,
            "Saved methods unsupported"
        );
        let base = reqwest::Url::parse(return_url)?;
        ensure!(
            base.scheme() == "https"
                && base.username().is_empty()
                && base.password().is_none()
                && base.fragment().is_none(),
            "Invalid return destination"
        );
        let mut success = base.clone();
        success.query_pairs_mut().append_pair("customer", local);
        // Stripe substitutes the literal placeholder after decoding the form value.
        let success = format!("{}&setup={{CHECKOUT_SESSION_ID}}", success.as_str());
        let mut form = pairs(&[
            ("mode", "setup"),
            ("customer", customer),
            ("currency", "usd"),
            (
                "payment_method_types[0]",
                if bank { "us_bank_account" } else { "card" },
            ),
            ("success_url", &success),
            ("cancel_url", return_url),
            ("metadata[business_customer]", local),
            ("metadata[purpose]", "save_only"),
        ]);
        if bank {
            form.extend(pairs(&[(
                "payment_method_options[us_bank_account][financial_connections][permissions][0]",
                "payment_method",
            )]));
        }
        let result = self
            .transport
            .stripe(Method::POST, "/checkout/sessions", key, &form)
            .await?;
        let url = field(&result, "url")?;
        transport::trusted_url(url, "checkout.stripe.com")?;
        Ok(url.into())
    }
    async fn confirm_method(&self, customer: &str, local: &str, session: &str) -> Result<String> {
        ensure!(
            self.capabilities().saved_methods,
            "Saved methods unsupported"
        );
        safe_id(session)?;
        let checkout = self
            .transport
            .stripe(
                Method::GET,
                &format!("/checkout/sessions/{session}"),
                "",
                &[],
            )
            .await?;
        ensure!(
            checkout["mode"] == "setup"
                && checkout["status"] == "complete"
                && checkout["customer"] == customer
                && checkout["metadata"]["business_customer"] == local
                && checkout["metadata"]["purpose"] == "save_only",
            "Setup not complete for this account"
        );
        let intent = field(&checkout, "setup_intent")?;
        safe_id(intent)?;
        let setup = self
            .transport
            .stripe(Method::GET, &format!("/setup_intents/{intent}"), "", &[])
            .await?;
        ensure!(
            setup["status"] == "succeeded" && setup["customer"] == customer,
            "Payment method not verified"
        );
        let method = field(&setup, "payment_method")?;
        self.method(customer, method).await?;
        Ok(method.into())
    }
    async fn detach_method(&self, customer: &str, method: &str, key: &str) -> Result<()> {
        ensure!(
            self.capabilities().saved_methods,
            "Saved methods unsupported"
        );
        safe_id(method)?;
        let current = self
            .transport
            .stripe(Method::GET, &format!("/payment_methods/{method}"), "", &[])
            .await?;
        ensure!(
            current["object"] == "payment_method"
                && current["id"] == method
                && current["livemode"].as_bool() == Some(self.transport.live)
                && current
                    .as_object()
                    .is_some_and(|o| o.contains_key("customer")),
            "Invalid payment method observation"
        );
        // Stripe's detached methods are terminal. A lost acknowledgement can be
        // reconciled without deleting a method now owned by another customer.
        if current["customer"].is_null() {
            return Ok(());
        }
        self.saved(&current, customer)?;
        self.transport
            .stripe(
                Method::POST,
                &format!("/payment_methods/{method}/detach"),
                key,
                &[],
            )
            .await?;
        Ok(())
    }
    async fn refund(&self, invoice: &str, key: &str, amount: i64) -> Result<RefundObservation> {
        self.create_refund(invoice, key, amount).await
    }
    async fn find_refund(&self, invoice: &str, key: &str) -> Result<Option<RefundObservation>> {
        self.lookup_refund(invoice, key).await
    }
    async fn history(&self, external: &str) -> Result<Value> {
        if !self.capabilities().payment_history {
            return Ok(
                json!({"supported":false,"message":"This financial plugin does not expose detailed payment history."}),
            );
        }
        let invoice = self.transport.observe(self.provider, external).await?;
        self.transport
            .payment_history(self.provider, &invoice)
            .await
    }
}
