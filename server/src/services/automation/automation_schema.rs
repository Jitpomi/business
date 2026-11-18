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
            "settings",
            "job"
        ]
        .contains(&kind),
        "Unknown record kind"
    );
    Ok(())
}
pub fn limit() -> usize {
    100
}

use crate::automation::model::{Schedule, Workflow};
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Command {
    OauthBegin {
        id: String,
        connector: String,
    },
    OauthFinish {
        state: String,
        code: Option<String>,
        #[serde(default)]
        denied: bool,
    },
    List {
        kind: String,
        after: Option<String>,
        #[serde(default = "limit")]
        limit: usize,
    },
    Inspect {
        kind: String,
        id: String,
    },
    Trigger {
        id: String,
        workflow: String,
        enabled: bool,
    },
    ConfigureBilling {
        settings: crate::automation::managed::Settings,
    },
    Connect {
        id: String,
        connector: String,
        config: Value,
        #[serde(default)]
        secrets: Value,
    },
    Verify {
        id: String,
    },
    Disconnect {
        id: String,
    },
    Publish {
        workflow: Workflow,
    },
    Run {
        workflow: String,
        key: String,
        #[serde(default)]
        input: Value,
    },
    Schedule {
        schedule: Schedule,
    },
    Approve {
        id: String,
    },
    Cancel {
        id: String,
        #[serde(default)]
        evidence: Option<String>,
    },
    Resume {
        id: String,
    },
    Resolve {
        id: String,
        evidence: String,
        output: Value,
    },
}
