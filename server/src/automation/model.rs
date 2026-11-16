use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Read,
    Idempotent,
    AtMostOnce,
}
#[derive(Clone, Debug, Serialize)]
pub struct ActionSpec {
    pub name: String,
    pub effect: Effect,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub connector: String,
    pub revision: String,
    pub config: Value,
    pub secret_ref: String,
    pub enabled: bool,
    pub verified: bool,
    #[serde(default)]
    pub verification: Option<String>,
}
impl Connection {
    pub fn public(&self) -> Value {
        serde_json::json!({"id":self.id,"connector":self.connector,"revision":self.revision,"config":self.config,"enabled":self.enabled,"verified":self.verified,"verification":self.verification})
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Action {
        id: String,
        connection: String,
        action: String,
        #[serde(default)]
        input: Value,
    },
    Transform {
        id: String,
        value: Value,
    },
    Condition {
        id: String,
        left: Value,
        equals: Value,
    },
    Delay {
        id: String,
        seconds: i64,
    },
    Approval {
        id: String,
    },
}
impl Step {
    pub fn resolve(&self, input: &Value, outputs: &BTreeMap<String, Value>) -> Result<Self> {
        let mut step = self.clone();
        match &mut step {
            Self::Action {
                input: template, ..
            } => *template = bind(template, input, outputs)?,
            Self::Transform { value, .. } => *value = bind(value, input, outputs)?,
            Self::Condition { left, equals, .. } => {
                *left = bind(left, input, outputs)?;
                *equals = bind(equals, input, outputs)?;
            }
            _ => {}
        }
        Ok(step)
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Action { id, .. }
            | Self::Transform { id, .. }
            | Self::Condition { id, .. }
            | Self::Delay { id, .. }
            | Self::Approval { id } => id,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub id: String,
    #[serde(default)]
    pub revision: String,
    pub name: String,
    pub enabled: bool,
    pub steps: Vec<Step>,
}
impl Workflow {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 120,
            "Workflow name required"
        );
        ensure!(
            !self.steps.is_empty() && self.steps.len() <= 64,
            "A workflow requires 1..64 steps"
        );
        let mut ids = std::collections::HashSet::new();
        for s in &self.steps {
            identifier(s.id())?;
            ensure!(!ids.contains(s.id()), "Duplicate step ID");
            match s {
                Step::Action {
                    connection,
                    action,
                    input,
                    ..
                } => {
                    identifier(connection)?;
                    identifier(action)?;
                    references(input, &ids)?;
                }
                Step::Transform { value, .. } => references(value, &ids)?,
                Step::Condition { left, equals, .. } => {
                    references(left, &ids)?;
                    references(equals, &ids)?;
                }
                _ => {}
            }
            ids.insert(s.id());
            if let Step::Delay { seconds, .. } = s {
                ensure!((0..=31_536_000).contains(seconds), "Invalid wait duration");
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub key: String,
    pub count: u32,
    pub effect: Effect,
    pub started: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub owner: String,
    pub id: String,
    pub workflow: Workflow,
    pub input: Value,
    pub outputs: BTreeMap<String, Value>,
    pub cursor: usize,
    pub status: String,
    pub wake: i64,
    pub attempt: Option<Attempt>,
    pub approved: Vec<String>,
    pub connections: BTreeMap<String, String>,
    pub last_error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    pub id: String,
    pub workflow: String,
    pub every_seconds: i64,
    pub next: i64,
    #[serde(default)]
    pub sequence: u64,
    pub input: Value,
    pub enabled: bool,
}
pub fn identifier(v: &str) -> Result<()> {
    ensure!(
        !v.is_empty()
            && v.len() <= 100
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid automation identifier"
    );
    Ok(())
}
/// Exact JSON references, never code evaluation or interpolated credentials.
pub fn bind(value: &Value, input: &Value, outputs: &BTreeMap<String, Value>) -> Result<Value> {
    fn charge(v: &Value, budget: &mut usize) -> Result<()> {
        let size = serde_json::to_vec(v)?.len();
        ensure!(size <= *budget, "Mapped value exceeds step output limit");
        *budget -= size;
        Ok(())
    }
    fn walk(
        v: &Value,
        input: &Value,
        outputs: &BTreeMap<String, Value>,
        budget: &mut usize,
        depth: usize,
    ) -> Result<Value> {
        ensure!(depth <= 64, "Mapping is too deeply nested");
        match v {
            Value::Object(o) if o.len() == 1 && o.contains_key("$ref") => {
                let r = o["$ref"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid reference"))?;
                let target = if let Some(p) = r.strip_prefix("input") {
                    input.pointer(p)
                } else if let Some(p) = r.strip_prefix("steps/") {
                    let (id, path) = p
                        .split_once('/')
                        .map(|(id, p)| (id, format!("/{p}")))
                        .unwrap_or((p, String::new()));
                    outputs.get(id).and_then(|v| v.pointer(&path))
                } else {
                    None
                }
                .ok_or_else(|| anyhow::anyhow!("Reference is unavailable"))?;
                charge(target, budget)?;
                Ok(target.clone())
            }
            Value::Object(o) => {
                ensure!(*budget >= 2, "Mapped value too large");
                *budget -= 2;
                let mut out = serde_json::Map::new();
                for (k, v) in o {
                    let overhead = serde_json::to_vec(k)?.len() + 2;
                    ensure!(*budget >= overhead, "Mapped value too large");
                    *budget -= overhead;
                    out.insert(k.clone(), walk(v, input, outputs, budget, depth + 1)?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(a) => {
                ensure!(*budget >= a.len() + 2, "Mapped value too large");
                *budget -= a.len() + 2;
                Ok(Value::Array(
                    a.iter()
                        .map(|v| walk(v, input, outputs, budget, depth + 1))
                        .collect::<Result<_>>()?,
                ))
            }
            _ => {
                charge(v, budget)?;
                Ok(v.clone())
            }
        }
    }
    walk(value, input, outputs, &mut 262144, 0)
}

fn references(value: &Value, previous: &std::collections::HashSet<&str>) -> Result<()> {
    match value {
        Value::Object(o) if o.len() == 1 && o.contains_key("$ref") => {
            let r = o["$ref"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid reference"))?;
            if r == "input" || r.starts_with("input/") {
                return Ok(());
            }
            let rest = r
                .strip_prefix("steps/")
                .ok_or_else(|| anyhow::anyhow!("Unknown reference namespace"))?;
            let id = rest.split('/').next().unwrap_or("");
            ensure!(
                previous.contains(id),
                "References must name an earlier step"
            );
        }
        Value::Object(o) => {
            for v in o.values() {
                references(v, previous)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                references(v, previous)?;
            }
        }
        _ => {}
    }
    Ok(())
}
