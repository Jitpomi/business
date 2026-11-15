//! Composition root for trusted, in-process plugins. No secrets enter business records.
use super::{
    builtin_transport::{Provider, Providers},
    BuiltinFinancial, Resend,
};
use crate::finance::{TenantRegistry, TenantRuntime};
use anyhow::{ensure, Result};
use serde::Deserialize;
use std::{collections::BTreeSet, sync::Arc};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    tenant_id: String,
    default_financial: Option<String>,
    webhook_secret: Option<String>,
    #[serde(default = "sandbox")]
    sandbox: bool,
    mercury_destination: Option<String>,
    #[serde(default)]
    mercury_card_enabled: bool,
    #[serde(default)]
    mercury_ach_debit_enabled: bool,
    scheduler_secret: Option<String>,
    reminder_days: Option<i64>,
}
fn sandbox() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mail {
    tenant_id: String,
    plugin: String,
    token: String,
    from: String,
    #[serde(default)]
    allowed_test_recipients: Vec<String>,
}
fn json<T: serde::de::DeserializeOwned>(key: &str) -> Result<Vec<T>> {
    Ok(serde_json::from_str(
        &std::env::var(key).unwrap_or_else(|_| "[]".into()),
    )?)
}
pub fn registry() -> Result<Arc<TenantRegistry>> {
    let settings: Vec<Settings> = json("TENANT_FINANCIAL_SETTINGS_JSON")?;
    let mail: Vec<Mail> = json("TENANT_NOTIFICATION_PLUGINS_JSON")?;
    let stripes: Vec<crate::plugins::stripe::auth_stripe::auth_stripe_schema::Connection> =
        json("STRIPE_CONNECTIONS_JSON")?;
    let mercuries: Vec<crate::plugins::mercury::auth_mercury::auth_mercury_schema::Connection> =
        json("MERCURY_CONNECTIONS_JSON")?;
    let mut tenants = BTreeSet::new();
    for tenant in settings
        .iter()
        .map(|v| &v.tenant_id)
        .chain(mail.iter().map(|v| &v.tenant_id))
        .chain(stripes.iter().map(|v| &v.tenant_id))
        .chain(mercuries.iter().map(|v| &v.tenant_id))
    {
        tenants.insert(tenant.clone());
    }
    let mut registry = TenantRegistry::default();
    for tenant in tenants {
        let tenant = crate::billing::billing_schema::id(&tenant)?;
        let setting = settings.iter().find(|s| s.tenant_id == tenant);
        let mut runtime =
            TenantRuntime::new(tenant.clone(), setting.map(|s| s.sandbox).unwrap_or(true));
        if let Some(s) = setting {
            runtime.scheduler_secret = s.scheduler_secret.clone();
            runtime.reminder_days = s.reminder_days.unwrap_or(7);
            ensure!(
                (1..=30).contains(&runtime.reminder_days),
                "Reminder interval must be 1..30 days"
            );
        }
        let transport = Providers::new(
            &tenant,
            stripes.clone(),
            mercuries.clone(),
            !runtime.sandbox,
            setting.and_then(|s| s.mercury_destination.clone()),
        )?
        .with_mercury_payment_options(
            setting.is_some_and(|s| s.mercury_card_enabled),
            setting.is_some_and(|s| s.mercury_ach_debit_enabled),
        );
        if stripes.iter().any(|c| c.tenant_id == tenant && c.write) {
            runtime.register(
                "stripe",
                Arc::new(BuiltinFinancial {
                    transport: transport.clone(),
                    provider: Provider::Stripe,
                    webhook_secret: settings
                        .iter()
                        .find(|s| s.tenant_id == tenant)
                        .and_then(|s| s.webhook_secret.clone()),
                }),
            )?;
        }
        if mercuries.iter().any(|c| c.tenant_id == tenant && c.write) {
            runtime.register(
                "mercury",
                Arc::new(BuiltinFinancial {
                    transport,
                    provider: Provider::Mercury,
                    webhook_secret: None,
                }),
            )?;
        }
        let matches: Vec<_> = settings.iter().filter(|s| s.tenant_id == tenant).collect();
        ensure!(matches.len() <= 1, "Duplicate tenant financial settings");
        if let Some(setting) = matches.first() {
            if let Some(id) = &setting.default_financial {
                runtime.financial(id)?;
                runtime.default_financial = Some(id.clone());
            }
        }
        let notifications: Vec<_> = mail.iter().filter(|m| m.tenant_id == tenant).collect();
        ensure!(
            notifications.len() <= 1,
            "Duplicate tenant notification plugin"
        );
        if let Some(m) = notifications.first() {
            ensure!(
                m.plugin == "resend",
                "Unknown built-in notification plugin; register an implementation at composition"
            );
            runtime.notification = Some(Arc::new(Resend::new(m.token.clone(), m.from.clone())?));
            runtime.allowed_test_recipients = m.allowed_test_recipients.clone();
        }
        registry.register(runtime)?;
    }
    Ok(Arc::new(registry))
}
pub fn engine(
    state: Arc<crate::typedb::TypeDBState>,
    tenant: &str,
) -> Result<crate::billing::engine::Engine> {
    let registry = registry()?;
    let runtime = registry
        .get(tenant)
        .unwrap_or_else(|_| Arc::new(TenantRuntime::new(tenant.into(), true)));
    crate::billing::engine::Engine::new(state, tenant, runtime)
}

pub fn transport(tenant: &str) -> Result<Providers> {
    let settings: Vec<Settings> = json("TENANT_FINANCIAL_SETTINGS_JSON")?;
    let setting = settings.iter().find(|s| s.tenant_id == tenant);
    Ok(Providers::new(
        tenant,
        json("STRIPE_CONNECTIONS_JSON")?,
        json("MERCURY_CONNECTIONS_JSON")?,
        !setting.map(|s| s.sandbox).unwrap_or(true),
        setting.and_then(|s| s.mercury_destination.clone()),
    )?
    .with_mercury_payment_options(
        setting.is_some_and(|s| s.mercury_card_enabled),
        setting.is_some_and(|s| s.mercury_ach_debit_enabled),
    ))
}

pub fn register_services(
    builder: &mut dog_core::DogAppBuilder<serde_json::Value, crate::BusinessParams>,
    authentication: Arc<dog_auth::AuthenticationService<crate::BusinessParams>>,
) -> Result<()> {
    use crate::plugins::{mercury, stripe};
    if let Some(credentials) = mercury::auth_mercury::AuthMercuryState::from_env()? {
        let state = mercury::adapters::mercury_adapter::MercuryState::new(Arc::new(
            mercury::adapters::mercury_adapter::MercuryHttp::new()?,
        ));
        mercury::configure(
            builder,
            authentication.clone(),
            Arc::new(state),
            Arc::new(credentials),
        )?;
    }
    if let Some(credentials) = stripe::auth_stripe::AuthStripeState::from_env()? {
        let state = stripe::adapters::stripe_adapter::StripeState::new(Arc::new(
            stripe::adapters::stripe_adapter::StripeHttp::new()?,
        ));
        stripe::configure(
            builder,
            authentication,
            Arc::new(state),
            Arc::new(credentials),
        )?;
    }
    Ok(())
}
