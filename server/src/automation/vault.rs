//! Authenticated encryption; tenant and connection version are authenticated context.
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
#[derive(Clone)]
pub struct Vault(
    Option<Aes256Gcm>,
    std::sync::Arc<dyn super::secret_store::SecretStore>,
);
impl Default for Vault {
    fn default() -> Self {
        Self(
            None,
            std::sync::Arc::new(super::secret_store::Memory::default()),
        )
    }
}
impl Vault {
    pub fn from_env() -> Result<Self> {
        match std::env::var("AUTOMATION_MASTER_KEY_B64") {
            Ok(v) => Self::with_store(&v, super::secret_store::configured()?),
            Err(_) => Ok(Self::default()),
        }
    }
    /// Explicit ephemeral store for embedded hosts/tests. Runtime uses from_env.
    pub fn new(encoded: &str) -> Result<Self> {
        Self::with_store(
            encoded,
            std::sync::Arc::new(super::secret_store::Memory::default()),
        )
    }
    pub fn with_store(
        encoded: &str,
        store: std::sync::Arc<dyn super::secret_store::SecretStore>,
    ) -> Result<Self> {
        let key = STANDARD.decode(encoded)?;
        ensure!(
            key.len() == 32,
            "Automation master key must decode to 32 bytes"
        );
        Ok(Self(
            Some(
                Aes256Gcm::new_from_slice(&key)
                    .map_err(|_| anyhow::anyhow!("Invalid vault key"))?,
            ),
            store,
        ))
    }
    pub async fn seal(&self, context: &str, value: &serde_json::Value) -> Result<String> {
        let cipher = self.0.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Configure the automation vault before connecting systems")
        })?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let plaintext = serde_json::to_vec(value)?;
        let encrypted = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("Secret encryption failed"))?;
        let mut data = nonce.to_vec();
        data.extend(encrypted);
        let id = uuid::Uuid::new_v4().to_string();
        self.1.put(&id, &STANDARD.encode(data)).await?;
        Ok(format!("secret:{id}"))
    }
    pub async fn open(&self, context: &str, value: &str) -> Result<serde_json::Value> {
        let id = value.strip_prefix("secret:").ok_or_else(|| {
            anyhow::anyhow!("Inline credentials require explicit migration before use")
        })?;
        let encrypted = self.1.get(id).await?;
        self.decrypt(context, &encrypted)
    }
    fn decrypt(&self, context: &str, value: &str) -> Result<serde_json::Value> {
        let cipher = self
            .0
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Automation vault unavailable"))?;
        let data = STANDARD.decode(value)?;
        ensure!(data.len() >= 28, "Invalid encrypted credential");
        let bytes = cipher
            .decrypt(
                Nonce::from_slice(&data[..12]),
                Payload {
                    msg: &data[12..],
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("Credential authentication failed"))?;
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub(crate) async fn import_inline(&self, context: &str, value: &str) -> Result<String> {
        let _ = self.decrypt(context, value)?;
        let id = uuid::Uuid::new_v4().to_string();
        self.1.put(&id, value).await?;
        Ok(format!("secret:{id}"))
    }
}
