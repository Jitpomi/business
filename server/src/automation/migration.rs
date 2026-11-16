//! Explicit, additive import. Existing provider aliases and invoice bindings stay intact.
use super::{
    engine::{now, Engine},
    managed::Settings,
    model::{Schedule, Step, Workflow},
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
fn rows(key: &str) -> Result<Vec<Value>> {
    Ok(serde_json::from_str(
        &std::env::var(key).unwrap_or_else(|_| "[]".into()),
    )?)
}
async fn connection(e: &Engine, id: &str, kind: &str, config: Value, secrets: Value) -> Result<()> {
    if e.store.get("connection", id).await?.is_some() {
        let (_, old, existing) = e.connection(id).await?;
        ensure!(
            old.connector == kind && old.config == config && existing == secrets && old.enabled,
            "Existing connection differs; import will not overwrite it"
        );
    } else {
        e.connect(id, kind, config, secrets).await?;
    }
    e.verify_connection(id).await?;
    Ok(())
}
pub async fn import(e: &Engine, owner: &str) -> Result<()> {
    e.store.owner(owner).await?;
    let tenant = &e.store.tenant;
    let settings: Vec<_> = rows("TENANT_FINANCIAL_SETTINGS_JSON")?
        .into_iter()
        .filter(|v| v["tenant_id"] == tenant.as_str())
        .collect();
    ensure!(settings.len() <= 1, "Duplicate settings");
    let setting = settings.first().cloned().unwrap_or_else(|| json!({}));
    let sandbox = setting["sandbox"].as_bool().unwrap_or(true);
    for (kind, env) in [
        ("stripe", "STRIPE_CONNECTIONS_JSON"),
        ("mercury", "MERCURY_CONNECTIONS_JSON"),
    ] {
        let matches: Vec<_> = rows(env)?
            .into_iter()
            .filter(|v| v["tenant_id"] == tenant.as_str() && v["write"] == true)
            .collect();
        ensure!(
            matches.len() <= 1,
            "Ambiguous provider account; import stopped"
        );
        if let Some(old) = matches.first() {
            let old_sandbox = matches!(old["environment"].as_str(), Some("test" | "sandbox"));
            ensure!(
                old_sandbox == sandbox,
                "Provider environment differs from tenant settings"
            );
            let config = if kind == "stripe" {
                json!({"sandbox":sandbox,"connected_account":old["connected_account"]})
            } else {
                json!({"sandbox":sandbox,"destination":setting["mercury_destination"],"card_enabled":setting["mercury_card_enabled"]==true,"ach_debit_enabled":setting["mercury_ach_debit_enabled"]==true})
            };
            let mut secrets = json!({"token":old["token"]});
            if kind == "stripe" && setting["webhook_secret"].is_string() {
                secrets["webhook_secret"] = setting["webhook_secret"].clone();
            }
            connection(e, kind, kind, config, secrets).await?;
        }
    }
    let mail: Vec<_> = rows("TENANT_NOTIFICATION_PLUGINS_JSON")?
        .into_iter()
        .filter(|v| v["tenant_id"] == tenant.as_str())
        .collect();
    ensure!(mail.len() <= 1, "Ambiguous notification connection");
    let mut recipients = vec![];
    if let Some(m) = mail.first() {
        ensure!(
            m["plugin"] == "resend",
            "Install the notification adapter before importing"
        );
        recipients = serde_json::from_value(
            m.get("allowed_test_recipients")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?;
        connection(
            e,
            "resend",
            "resend",
            json!({"from":m["from"],"sandbox":sandbox,"allowed_test_recipients":recipients}),
            json!({"token":m["token"]}),
        )
        .await?;
    }
    let settings = Settings {
        sandbox,
        default_financial: setting["default_financial"].as_str().map(str::to_owned),
        notification: mail.first().map(|_| "resend".into()),
        reminder_days: setting["reminder_days"].as_i64().unwrap_or(7),
        allowed_test_recipients: recipients,
    };
    let value = serde_json::to_value(&settings)?;
    if let Some(old) = e.store.get("settings", "billing").await? {
        ensure!(
            old.value == value,
            "Existing billing settings differ; import stopped"
        );
    } else {
        e.configure_billing(settings).await?;
    }
    connection(e, "billing", "billing", json!({}), Value::Null).await?;
    if let Some(existing) = e.store.get("workflow", "billing-cycle").await? {
        let w: Workflow = serde_json::from_value(existing.value["workflow"].clone())?;
        ensure!(
            existing.value["owner"] == owner
                && w.enabled
                && serde_json::to_value(&w.steps)?
                    == json!([{"kind":"action","id":"billing","connection":"billing","action":"tick","input":null}]),
            "Existing billing-cycle workflow differs; import stopped"
        );
    } else {
        e.publish(
            Workflow {
                id: "billing-cycle".into(),
                revision: String::new(),
                name: "Invoices, payments, receipts and reminders".into(),
                enabled: true,
                steps: vec![Step::Action {
                    id: "billing".into(),
                    connection: "billing".into(),
                    action: "tick".into(),
                    input: Value::Null,
                }],
            },
            owner,
        )
        .await?;
    }
    if let Some(existing) = e.store.get("schedule", "billing-cycle").await? {
        let s: Schedule = serde_json::from_value(existing.value)?;
        ensure!(
            s.workflow == "billing-cycle"
                && s.every_seconds == 60
                && s.input.is_null()
                && s.enabled,
            "Existing billing schedule differs; import stopped"
        );
    } else {
        e.schedule(Schedule {
            id: "billing-cycle".into(),
            workflow: "billing-cycle".into(),
            every_seconds: 60,
            next: now(),
            sequence: 0,
            input: Value::Null,
            enabled: true,
        })
        .await?;
    }
    Ok(())
}
