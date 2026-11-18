//! Application-wide transport admission and DogRS hook registration.
//! Service authentication hooks stay with each service; TypeDB decides authorization.
use crate::BusinessParams;
use anyhow::Result;
use dog_core::DogAppBuilder;
use serde_json::Value;
pub fn register_global_hooks(_builder: &mut DogAppBuilder<Value, BusinessParams>) -> Result<()> {
    // No application-wide DogRS hooks are required. HTTP admission is transport middleware.
    Ok(())
}
pub(crate) use crate::admission::{external_guard, guard};
