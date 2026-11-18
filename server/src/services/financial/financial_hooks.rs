//! Financial proxy authentication; authorization remains in native TypeDB policies.
use crate::BusinessParams;
use dog_core::DogAppBuilder;
use serde_json::Value;
use std::sync::Arc;
pub fn register_hooks(
    builder: &mut DogAppBuilder<Value, BusinessParams>,
    auth: Arc<dog_auth::AuthenticationService<BusinessParams>>,
) {
    builder.service_hooks("financial", |h| {
        h.before_all(Arc::new(
            dog_auth::hooks::AuthenticateHook::new(auth, vec!["jwt".into()]).strict(),
        ));
    });
}
