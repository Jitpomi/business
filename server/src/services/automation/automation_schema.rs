pub use crate::services::actor;
pub fn kind(kind: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        [
            "connection",
            "workflow",
            "workflow_version",
            "run",
            "schedule",
            "trigger",
            "settings"
        ]
        .contains(&kind),
        "Unknown record kind"
    );
    Ok(())
}
pub fn limit() -> usize {
    100
}
