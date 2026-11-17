//! Encode typed facts atomically beside the preserved payload. TypeQL owns policy.
use super::model::{Connection, Run, Step, Workflow};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
pub const ATTRIBUTES: &[&str] = &[
    "auto_owner",
    "auto_cursor",
    "auto_steps",
    "auto_attempts",
    "auto_effect",
    "auto_enabled",
    "auto_verified",
    "auto_binding_revision",
    "auto_definition",
    "auto_step",
    "auto_approved",
    "auto_approval_history",
];
pub(crate) fn facts(kind: &str, value: &Value, status: &str) -> Result<BTreeMap<String, Value>> {
    let v = match kind {
        "run" => {
            let r: Run = serde_json::from_value(value.clone())?;
            ensure!(
                r.status == status && r.cursor <= r.workflow.steps.len(),
                "Run payload and native state disagree"
            );
            let (count, effect) = r
                .attempt
                .as_ref()
                .map(|a| {
                    (
                        a.count,
                        serde_json::to_value(a.effect).expect("effect serializes"),
                    )
                })
                .unwrap_or((0, json!("")));
            let step = match r.workflow.steps.get(r.cursor) {
                Some(Step::Action { .. }) => "action",
                Some(Step::Transform { .. }) => "transform",
                Some(Step::Condition { .. }) => "condition",
                Some(Step::Delay { .. }) => "delay",
                Some(Step::Approval { .. }) => "approval",
                None => "done",
            };
            json!({"auto_step":step,"auto_approved":approved(&r),"auto_approval_history":approval_history(&r)?,"auto_owner":r.owner,"auto_cursor":r.cursor,"auto_steps":r.workflow.steps.len(),"auto_attempts":count,"auto_effect":effect,"auto_definition":definition(&r)?})
        }
        "workflow" | "workflow_version" => {
            let w: Workflow = serde_json::from_value(value["workflow"].clone())?;
            json!({"auto_owner":value["owner"].as_str().ok_or_else(||anyhow::anyhow!("Workflow owner missing"))?,"auto_enabled":w.enabled})
        }
        "connection" => {
            let c: Connection = serde_json::from_value(value.clone())?;
            json!({"auto_enabled":c.enabled,"auto_verified":c.verified,"auto_binding_revision":c.revision})
        }
        "schedule" | "trigger" => {
            json!({"auto_enabled":value["enabled"].as_bool().ok_or_else(||anyhow::anyhow!("Enabled flag missing"))?,"auto_definition":origin_definition(kind,value)?})
        }
        _ => json!({}),
    };
    Ok(serde_json::from_value(v)?)
}
pub(crate) fn attributes(kind: &str, value: &Value, status: &str) -> Result<String> {
    Ok(facts(kind, value, status)?
        .into_iter()
        .map(|(name, value)| format!(",has {name} {value}"))
        .collect())
}
fn digest(value: &impl serde::Serialize) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
pub(crate) fn definition(run: &Run) -> Result<String> {
    digest(
        &json!({"owner":run.owner,"workflow":run.workflow,"input":run.input,"connections":run.connections}),
    )
}
pub(crate) fn approval_history(run: &Run) -> Result<String> {
    digest(&run.approved)
}
pub(crate) fn approved(run: &Run) -> bool {
    matches!(run.workflow.steps.get(run.cursor),Some(Step::Approval{id}) if run.approved.contains(id))
}
pub(crate) fn origin_definition(kind: &str, value: &Value) -> Result<String> {
    digest(&match kind {
        "schedule" => {
            json!({"workflow":value["workflow"],"input":value["input"],"every_seconds":value["every_seconds"]})
        }
        "trigger" => json!({"workflow":value["workflow"],"digest":value["digest"]}),
        _ => anyhow::bail!("Invalid event origin"),
    })
}
