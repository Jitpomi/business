//! Stripe maps invoice payments to refunds inside the tenant connection.
use super::{
    builtin_transport::{field, pairs, safe_id, Provider},
    BuiltinFinancial,
};
use crate::finance::{FinancialPlugin, RefundObservation, RefundState};
use anyhow::{ensure, Result};
use reqwest::Method;
use serde_json::Value;
impl BuiltinFinancial {
    async fn refund_payment(&self, invoice: &str) -> Result<(Value, String)> {
        ensure!(
            self.provider == Provider::Stripe,
            "Refunds unsupported by this financial plugin"
        );
        let observed = self.transport.observe(self.provider, invoice).await?;
        ensure!(observed["status"] == "paid", "Invoice is not settled");
        let rows = self
            .transport
            .list_history("/invoice_payments", "invoice", invoice)
            .await?;
        let paid: Vec<_> = rows.iter().filter(|v| v["status"] == "paid").collect();
        // Split/tendered invoice refunds need explicit allocation; never guess a payment.
        ensure!(
            paid.len() == 1,
            "Refund requires a single supported settled invoice payment"
        );
        let p = paid[0];
        ensure!(
            p["invoice"] == invoice && p["livemode"] == self.transport.live,
            "Invoice payment binding mismatch"
        );
        ensure!(
            p["payment"]["type"] == "payment_intent",
            "Unsupported invoice payment type"
        );
        let id = field(&p["payment"], "payment_intent")?;
        safe_id(id)?;
        let intent = self
            .transport
            .stripe(Method::GET, &format!("/payment_intents/{id}"), "", &[])
            .await?;
        ensure!(
            intent["customer"] == observed["customer"]
                && intent["status"] == "succeeded"
                && intent["currency"] == observed["currency"]
                && intent["amount_received"] == observed["total"],
            "Refund payment binding mismatch"
        );
        Ok((observed, id.into()))
    }
    fn refund_observation(
        &self,
        invoice: &Value,
        intent: &str,
        v: &Value,
    ) -> Result<RefundObservation> {
        ensure!(
            v["payment_intent"] == intent
                && v["metadata"]["business_invoice"] == invoice["metadata"]["business_invoice"],
            "Refund invoice binding mismatch"
        );
        let state = match field(v, "status")? {
            "succeeded" => RefundState::Succeeded,
            "failed" | "canceled" => RefundState::Failed,
            "pending" | "requires_action" => RefundState::Pending,
            _ => anyhow::bail!("Unrecognized refund status"),
        };
        Ok(RefundObservation {
            external_id: field(v, "id")?.into(),
            request_id: field(&v["metadata"], "business_refund")?.into(),
            invoice_id: field(invoice, "id")?.into(),
            customer: field(invoice, "customer")?.into(),
            amount_minor: v["amount"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Invalid refund amount"))?,
            currency: field(v, "currency")?.to_ascii_uppercase(),
            state,
        })
    }
    pub(super) async fn create_refund(
        &self,
        invoice: &str,
        key: &str,
        amount: i64,
    ) -> Result<RefundObservation> {
        self.ready()?;
        let (observed, intent) = self
            .refund_payment(invoice)
            .await
            .map_err(|_| crate::finance::RefundNotSubmitted)?;
        ensure!(amount > 0, "Refund must be positive");
        let result = self
            .transport
            .stripe(
                Method::POST,
                "/refunds",
                &format!("refund-{key}"),
                &pairs(&[
                    ("payment_intent", &intent),
                    ("amount", &amount.to_string()),
                    ("metadata[business_refund]", key),
                    (
                        "metadata[business_invoice]",
                        field(&observed["metadata"], "business_invoice")?,
                    ),
                ]),
            )
            .await
            .map_err(|error| {
                if error
                    .downcast_ref::<super::builtin_transport::Rejected>()
                    .is_some()
                {
                    anyhow::Error::new(crate::finance::RefundNotSubmitted)
                } else {
                    error
                }
            })?;
        self.refund_observation(&observed, &intent, &result)
    }
    pub(super) async fn lookup_refund(
        &self,
        invoice: &str,
        key: &str,
    ) -> Result<Option<RefundObservation>> {
        let (observed, intent) = self.refund_payment(invoice).await?;
        let rows = self
            .transport
            .list_history("/refunds", "payment_intent", &intent)
            .await?;
        let found: Vec<_> = rows
            .iter()
            .filter(|v| v["metadata"]["business_refund"] == key)
            .collect();
        ensure!(
            found.len() <= 1,
            "Duplicate provider refund references require review"
        );
        found
            .first()
            .map(|v| self.refund_observation(&observed, &intent, v))
            .transpose()
    }
}
