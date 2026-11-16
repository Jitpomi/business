//! Billing keeps its existing consent, provider binding and reconciliation rules.
use super::{
    connectors::{Connector, Invocation},
    model::{ActionSpec, Effect},
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::sync::Arc;
pub struct Billing {
    pub state: Arc<crate::typedb::TypeDBState>,
    pub registry: Arc<crate::finance::TenantRegistry>,
}
#[async_trait::async_trait]
impl Connector for Billing {
    fn timeout_seconds(&self) -> u64 {
        300
    }
    fn verification_method(&self) -> &str {
        "configuration"
    }
    fn name(&self) -> &str {
        "billing"
    }
    fn actions(&self) -> Vec<ActionSpec> {
        [
            "tick",
            "enqueue_due",
            "issue",
            "collect_due",
            "notify",
            "reconcile",
        ]
        .into_iter()
        .map(|name| ActionSpec {
            name: name.into(),
            effect: if name == "reconcile" {
                Effect::Read
            } else {
                Effect::AtMostOnce
            },
        })
        .collect()
    }

    fn validate_input(&self, _: &Value, action: &str, input: &Value) -> Result<()> {
        match action {
            "tick" | "enqueue_due" => {
                ensure!(
                    input.is_null() || input == &json!({}),
                    "Billing tick takes no account selector"
                );
            }
            "reconcile" | "issue" | "collect_due" | "notify" => {
                crate::billing::billing_schema::id(input["invoice_id"].as_str().unwrap_or(""))?;
            }
            _ => anyhow::bail!("Unknown billing action"),
        }
        Ok(())
    }
    fn validate(&self, c: &Value, s: &Value) -> Result<()> {
        ensure!(
            c == &json!({}) && s.is_null(),
            "Billing uses the tenant's financial connections"
        );
        Ok(())
    }
    async fn verify(&self, c: &Value, s: &Value) -> Result<()> {
        self.validate(c, s)
    }
    async fn execute(&self, ctx: &Invocation, action: &str, input: Value) -> Result<Value> {
        let e = crate::billing::engine::Engine::new(
            self.state.clone(),
            &ctx.tenant,
            self.registry.resolve(&ctx.tenant).await?,
        )?;
        match action {
            "tick" => e.tick().await,
            "enqueue_due" => Ok(json!({"scheduled":e.enqueue_due(super::engine::now()).await?})),
            "issue" | "collect_due" | "notify" => {
                let id = input["invoice_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invoice ID required"))?;
                match action {
                    "issue" => e.issue(id).await?,
                    "collect_due" => e.collect_due(id, super::engine::now()).await?,
                    _ => {
                        let sent = e.notify_invoice(id).await?;
                        return Ok(json!({"notices":sent}));
                    }
                }
                Ok(json!({"completed":true}))
            }
            "reconcile" => {
                let id = input["invoice_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invoice ID required"))?;
                e.reconcile(id).await?;
                Ok(json!({"reconciled":true}))
            }
            _ => anyhow::bail!("Unknown billing action"),
        }
    }
}
