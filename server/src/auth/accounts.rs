//! Optional local-account provider contract. Implementations own persistence outside TypeDB.
use anyhow::Result;
use serde_json::Value;
pub trait LocalAccounts: Send + Sync {
    fn lookup(&self, key: &str, by_login: bool) -> Result<Option<Value>>;
    fn import(&self, subject: &str, login: &str, hash: &str, version: &str) -> Result<()>;
    fn challenge(&self, key: &str) -> Result<Option<Value>>;
    fn issue(&self, key: &str, payload: &Value) -> Result<()>;
    fn consume(&self, key: &str) -> Result<bool>;
    fn reset(
        &self,
        key: &str,
        subject: &str,
        version: &str,
        hash: &str,
        new_version: &str,
    ) -> Result<()>;
    fn recent(&self, kind: &str, email: &str, cutoff: i64) -> Result<bool>;
    fn revoked(&self, key: &str) -> Result<bool>;
    fn revoke(&self, key: &str, expires: i64) -> Result<bool>;
}
