//! Resolve tenant-selected financial capabilities from encrypted connections on every operation.
use super::{
    connectors::{Catalog, Invocation},
    model::Connection,
    store::Store,
    vault::Vault,
};
use crate::finance::{RuntimeSource, TenantRuntime};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub sandbox: bool,
    pub default_financial: Option<String>,
    pub notification: Option<String>,
    pub reminder_days: i64,
    pub allowed_test_recipients: Vec<String>,
}
pub struct Managed {
    pub state: Arc<crate::typedb::TypeDBState>,
    pub catalog: Arc<Catalog>,
    pub vault: Vault,
}
#[async_trait::async_trait]
impl RuntimeSource for Managed {
    async fn resolve(&self, tenant: &str) -> Result<Arc<TenantRuntime>> {
        let store = Store::new(self.state.clone(), tenant)?;
        let Some(row) = store.get("settings", "billing").await? else {
            return Ok(Arc::new(TenantRuntime::new(tenant.into(), true)));
        };
        let settings: Settings = serde_json::from_value(row.value)?;
        ensure!(
            (1..=30).contains(&settings.reminder_days),
            "Invalid reminder interval"
        );
        let mut r = TenantRuntime::new(tenant.into(), settings.sandbox);
        r.reminder_days = settings.reminder_days;
        r.allowed_test_recipients = settings.allowed_test_recipients;
        for row in store.list("connection", 100).await? {
            let c: Connection = serde_json::from_value(row.value)?;
            if !c.enabled || !c.verified {
                continue;
            }
            let Ok(connector) = self.catalog.get(&c.connector) else {
                continue;
            };
            let secrets = if c.sealed.is_empty() {
                serde_json::Value::Null
            } else {
                self.vault
                    .open(&format!("{tenant}/{}/{}", c.id, c.revision), &c.sealed)?
            };
            let ctx = Invocation {
                tenant: tenant.into(),
                run: String::new(),
                step: String::new(),
                key: String::new(),
                config: c.config,
                secrets,
            };
            if let Some(financial) = connector.financial(&ctx)? {
                if ctx.config["sandbox"].as_bool() != Some(settings.sandbox) {
                    continue;
                }
                r.register(&c.id, financial)?;
            }
            if settings.notification.as_ref() == Some(&c.id) {
                r.notification = connector.notification(&ctx)?;
                ensure!(
                    r.notification.is_some(),
                    "Selected connection does not send notifications"
                );
            }
        }
        if let Some(id) = settings.default_financial {
            r.financial(&id)?;
            r.default_financial = Some(id);
        }
        if settings.notification.is_some() {
            ensure!(
                r.notification.is_some(),
                "Notification connection unavailable"
            );
        }
        Ok(Arc::new(r))
    }
}
impl super::engine::Engine {
    pub async fn configure_billing(&self, s: Settings) -> Result<serde_json::Value> {
        ensure!(
            (1..=30).contains(&s.reminder_days),
            "Reminder interval must be 1..30 days"
        );
        ensure!(
            s.allowed_test_recipients.len() <= 100,
            "Too many recipients"
        );
        for (id, financial) in [
            (s.default_financial.as_ref(), true),
            (s.notification.as_ref(), false),
        ] {
            if let Some(id) = id {
                let (_, c, secrets) = self.connection(id).await?;
                ensure!(c.enabled && c.verified, "Verify selected connections first");
                ensure!(
                    c.config["sandbox"].as_bool() == Some(s.sandbox),
                    "Connection environment mismatch"
                );
                let ctx = Invocation {
                    tenant: self.store.tenant.clone(),
                    run: String::new(),
                    step: String::new(),
                    key: String::new(),
                    config: c.config,
                    secrets,
                };
                let plugin = self.catalog.get(&c.connector)?;
                if financial {
                    plugin
                        .financial(&ctx)?
                        .ok_or_else(|| anyhow::anyhow!("Financial capability required"))?
                        .ready()?;
                } else {
                    ensure!(
                        plugin.notification(&ctx)?.is_some(),
                        "Notification capability required"
                    );
                }
            }
        }
        let value = serde_json::to_value(s)?;
        if let Some(old) = self.store.get("settings", "billing").await? {
            self.store.cas(&old, &value, "configured", 0).await?;
        } else {
            self.store
                .insert("settings", "billing", &value, "configured", 0)
                .await?;
        }
        Ok(value)
    }
}
