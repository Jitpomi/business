//! Authenticated encryption; tenant and connection version are authenticated context.
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
#[derive(Clone, Default)]
pub struct Vault(Option<Aes256Gcm>);
impl Vault {
    pub fn from_env() -> Result<Self> {
        match std::env::var("AUTOMATION_MASTER_KEY_B64") {
            Ok(v) => Self::new(&v),
            Err(_) => Ok(Self::default()),
        }
    }
    pub fn new(encoded: &str) -> Result<Self> {
        let key = STANDARD.decode(encoded)?;
        ensure!(
            key.len() == 32,
            "Automation master key must decode to 32 bytes"
        );
        Ok(Self(Some(
            Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow::anyhow!("Invalid vault key"))?,
        )))
    }
    pub fn seal(&self, context: &str, value: &serde_json::Value) -> Result<String> {
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
        Ok(STANDARD.encode(data))
    }
    pub fn open(&self, context: &str, value: &str) -> Result<serde_json::Value> {
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
}
