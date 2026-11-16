use dog_core::DogAppBuilder;
use serde_json::Value;
use std::sync::Arc;
pub(crate) mod automation;
pub(crate) mod data;
pub(crate) mod portal;
mod types;
pub use types::*;
pub(crate) mod authentication;

pub fn configure(
    builder: &mut DogAppBuilder<Value, BusinessParams>,
    auth: Arc<authentication::AuthService>,
    state: Arc<crate::typedb::TypeDBState>,
    registry: Arc<crate::finance::TenantRegistry>,
) -> anyhow::Result<()> {
    let plugins: Arc<crate::auth::Registry> = builder
        .get("authentication.plugins")
        .ok_or_else(|| anyhow::anyhow!("Authentication plugins missing"))?;
    builder.register_service(
        "authentication",
        Arc::new(
            authentication::authentication_service::PluginAuthentication {
                auth,
                plugins,
                state: state.clone(),
            },
        ),
    );
    authentication::authentication_shared::register_hooks(builder)?;
    builder.register_service("data", Arc::new(data::DataService::new(state.clone())));
    let authentication: Arc<dog_auth::AuthenticationService<BusinessParams>> = builder
        .get("authentication")
        .ok_or_else(|| anyhow::anyhow!("Authentication service missing"))?;
    data::data_shared::register_hooks(builder, authentication.clone())?;
    if std::env::var("BILLING_ENABLED").as_deref() == Ok("true") {
        builder.register_service(
            "portal",
            Arc::new(portal::PortalService::new(state, registry)),
        );
        portal::portal_shared::register_hooks(builder, authentication.clone())?;
    }
    if std::env::var("AUTOMATION_ENABLED").as_deref() == Ok("true") {
        let template = builder
            .get("automation.engine")
            .ok_or_else(|| anyhow::anyhow!("Automation engine missing"))?;
        builder.register_service(
            "automation",
            Arc::new(automation::AutomationService { template }),
        );
        automation::automation_shared::register_hooks(builder, authentication.clone())?;
    }
    crate::plugins::configuration::register_services(builder, authentication)?;
    Ok(())
}
