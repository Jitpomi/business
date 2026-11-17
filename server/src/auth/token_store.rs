//! Local-plugin session state is durable and outside the business graph.
use anyhow::Result;
use dog_auth::core::TokenStore;
use sha2::{Digest, Sha256};
pub struct LocalTokenStore {
    store: std::sync::Arc<dyn super::accounts::LocalAccounts>,
}
impl LocalTokenStore {
    pub fn new(store: std::sync::Arc<dyn super::accounts::LocalAccounts>) -> Self {
        Self { store }
    }
    fn key(issuer: &str, jti: &str) -> String {
        let mut hash = Sha256::new();
        hash.update((issuer.len() as u64).to_be_bytes());
        hash.update(issuer);
        hash.update(jti);
        format!("{:x}", hash.finalize())
    }
}
#[async_trait::async_trait]
impl TokenStore for LocalTokenStore {
    async fn is_revoked(&self, issuer: &str, jti: &str) -> Result<bool> {
        self.store.revoked(&Self::key(issuer, jti))
    }
    async fn revoke(&self, issuer: &str, jti: &str, expires: i64) -> Result<()> {
        self.store.revoke(&Self::key(issuer, jti), expires)?;
        Ok(())
    }
    async fn consume_refresh(&self, issuer: &str, jti: &str, expires: i64) -> Result<bool> {
        if expires <= chrono::Utc::now().timestamp() {
            return Ok(false);
        }
        self.store.revoke(&Self::key(issuer, jti), expires)
    }
}
#[cfg(test)]
#[path = "../../tests/unit/auth/token_store/tests.rs"]
mod tests;
