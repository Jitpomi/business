//! Validate the external-operation envelope before resolving tenant plugins.
use anyhow::{ensure, Result};
use serde_json::Value;
pub fn action(data: &Value) -> Result<&str> {
    let action = crate::billing::access::field(data, "action")?;
    ensure!(
        matches!(
            action,
            "checkout_configuration"
                | "checkout_options"
                | "history"
                | "payment_methods"
                | "setup_method"
                | "confirm_method"
                | "detach_method"
        ),
        dog_core::DogError::bad_request("Unsupported financial plugin operation").into_anyhow()
    );
    Ok(action)
}
