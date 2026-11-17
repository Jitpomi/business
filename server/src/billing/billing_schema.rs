use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Manual,
    Automatic,
}
pub fn word<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}
pub fn quoted(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}
pub fn id(value: &str) -> Result<String> {
    Ok(uuid::Uuid::parse_str(value)?.to_string())
}
