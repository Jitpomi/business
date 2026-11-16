pub use crate::services::actor;
use dog_core::DogError;
use serde_json::Value;
pub fn field<'a>(v: &'a Value, k: &str) -> anyhow::Result<&'a str> {
    v[k].as_str()
        .filter(|s| !s.is_empty() && s.len() < 512)
        .ok_or_else(|| DogError::bad_request("Required information is missing").into_anyhow())
}
