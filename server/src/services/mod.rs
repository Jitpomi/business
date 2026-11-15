use dog_core::DogAppBuilder;
use serde_json::Value;
use std::sync::Arc;
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
    builder.register_service("authentication", auth);
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
    crate::plugins::configuration::register_services(builder, authentication)?;
    Ok(())
}
