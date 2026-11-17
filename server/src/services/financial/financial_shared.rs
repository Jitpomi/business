//! Registration of the tenant financial proxy.
use crate::{finance::TenantRegistry, typedb::TypeDBState, BusinessParams};
use dog_core::DogAppBuilder;
use serde_json::Value;
use std::sync::Arc;
pub fn register(
    builder: &mut DogAppBuilder<Value, BusinessParams>,
    state: Arc<TypeDBState>,
    registry: Arc<TenantRegistry>,
    auth: Arc<dog_auth::AuthenticationService<BusinessParams>>,
) {
    builder.register_service(
        "financial",
        Arc::new(super::FinancialService::new(state, registry)),
    );
    super::financial_hooks::register_hooks(builder, auth);
}
