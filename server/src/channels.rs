//! Application event-channel configuration.
use crate::BusinessParams;
use anyhow::Result;
use dog_core::DogAppBuilder;
use serde_json::Value;
pub fn configure(_builder: &mut DogAppBuilder<Value, BusinessParams>) -> Result<()> {
    // Durable workflow events use TypeDB outboxes and dog-queue, not ephemeral channels.
    Ok(())
}
