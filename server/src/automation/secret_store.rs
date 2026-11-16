//! External secret persistence. TypeDB receives only opaque references.
use anyhow::{ensure, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
#[async_trait::async_trait]
pub trait SecretStore: Send + Sync {
    async fn put(&self, id: &str, ciphertext: &str) -> Result<()>;
    async fn get(&self, id: &str) -> Result<String>;
}
#[derive(Default)]
pub struct Memory(Mutex<HashMap<String, String>>);
#[async_trait::async_trait]
impl SecretStore for Memory {
    async fn put(&self, id: &str, value: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Secret store unavailable"))?
            .insert(id.into(), value.into());
        Ok(())
    }
    async fn get(&self, id: &str) -> Result<String> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Secret store unavailable"))?
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Secret reference unavailable"))
    }
}
pub struct Files(PathBuf);
/// HTTPS persistence for encrypted payloads. The provider never receives the master key.
pub struct Remote {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    token: String,
}
impl Remote {
    pub fn new(endpoint: &str, token: String) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let endpoint = reqwest::Url::parse(endpoint)?;
        ensure!(
            endpoint.scheme() == "https"
                && endpoint.host_str().is_some()
                && endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none()
                && endpoint.path() == "/",
            "Secret store requires an HTTPS origin"
        );
        ensure!(
            (32..=256).contains(&token.len())
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Invalid secret-store token"
        );
        Ok(Self {
            client: reqwest::Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            endpoint,
            token,
        })
    }
    fn url(&self, id: &str) -> Result<reqwest::Url> {
        Ok(self
            .endpoint
            .join(&format!("secrets/{}", uuid::Uuid::parse_str(id)?))?)
    }
}
#[async_trait::async_trait]
impl SecretStore for Remote {
    async fn put(&self, id: &str, value: &str) -> Result<()> {
        ensure!(value.len() <= 262144, "Secret exceeds limit");
        let response = self
            .client
            .put(self.url(id)?)
            .bearer_auth(&self.token)
            .header("content-type", "text/plain")
            .body(value.to_owned())
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Secret store transport failed"))?;
        ensure!(
            response.status() == reqwest::StatusCode::CREATED,
            "Secret store refused write ({})",
            response.status().as_u16()
        );
        Ok(())
    }
    async fn get(&self, id: &str) -> Result<String> {
        let mut response = self
            .client
            .get(self.url(id)?)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Secret store transport failed"))?;
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "Secret store refused read ({})",
            response.status().as_u16()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Secret store response failed"))?
        {
            ensure!(bytes.len() + chunk.len() <= 262144, "Secret exceeds limit");
            bytes.extend_from_slice(&chunk);
        }
        Ok(String::from_utf8(bytes)?)
    }
}
pub fn configured() -> Result<Arc<dyn SecretStore>> {
    match std::env::var("AUTOMATION_SECRET_STORE_URL") {
        Ok(url) => {
            ensure!(
                std::env::var_os("AUTOMATION_SECRET_STORE_DIR").is_none(),
                "Select exactly one secret store"
            );
            Ok(Arc::new(Remote::new(
                &url,
                std::env::var("AUTOMATION_SECRET_STORE_TOKEN")?,
            )?))
        }
        Err(_) => Files::configured(),
    }
}

impl Files {
    pub fn configured() -> Result<Arc<dyn SecretStore>> {
        let root = PathBuf::from(std::env::var("AUTOMATION_SECRET_STORE_DIR")?);
        ensure!(
            root.is_absolute() && root.is_dir(),
            "Create an absolute private secret-store directory explicitly"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                std::fs::metadata(&root)?.permissions().mode() & 0o077 == 0,
                "Secret-store directory must be private"
            );
            ensure!(
                !std::fs::symlink_metadata(&root)?.file_type().is_symlink(),
                "Secret-store directory cannot be a symlink"
            );
        }
        Ok(Arc::new(Self(root)))
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        let id = uuid::Uuid::parse_str(id)?.to_string();
        Ok(self.0.join(format!("secret-{id}")))
    }
}
#[async_trait::async_trait]
impl SecretStore for Files {
    async fn put(&self, id: &str, value: &str) -> Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(self.path(id)?)?;
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
        #[cfg(unix)]
        std::fs::File::open(&self.0)?.sync_all()?;
        Ok(())
    }
    async fn get(&self, id: &str) -> Result<String> {
        let path = self.path(id)?;
        ensure!(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "Secret references cannot be symlinks"
        );
        let value = std::fs::read_to_string(path)?;
        ensure!(value.len() <= 262144, "Secret exceeds limit");
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_rejects_unsafe_endpoints_and_credentials() {
        for url in [
            "http://store.example/",
            "https://user:password@store.example/",
            "https://store.example/?token=private",
            "https://store.example/#fragment",
            "https://store.example/path",
        ] {
            assert!(Remote::new(url, "x".repeat(43)).is_err());
        }
        assert!(Remote::new("https://store.example/", "short".into()).is_err());
    }
    #[tokio::test]
    #[ignore = "Explicit development secret-store endpoint; writes synthetic ciphertext only"]
    async fn remote_ciphertext_roundtrip_and_denials() -> Result<()> {
        dotenvy::from_path(std::env::var("BUSINESS_SECRET_TEST_ENV")?)?;
        let url = std::env::var("AUTOMATION_SECRET_STORE_URL")?;
        let token = std::env::var("AUTOMATION_SECRET_STORE_TOKEN")?;
        let store = Arc::new(Remote::new(&url, token.clone())?);
        let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let vault = crate::automation::vault::Vault::with_store(key, store.clone())?;
        let value = serde_json::json!({"synthetic":"durable-store-test"});
        let reference = vault.seal("development/test/revision", &value).await?;
        let restarted =
            crate::automation::vault::Vault::with_store(key, Arc::new(Remote::new(&url, token)?))?;
        ensure!(
            restarted
                .open("development/test/revision", &reference)
                .await?
                == value
        );
        ensure!(restarted
            .open("other-tenant/test/revision", &reference)
            .await
            .is_err());
        let id = reference.strip_prefix("secret:").unwrap();
        let ciphertext = store.get(id).await?;
        ensure!(
            store.put(id, &ciphertext).await.is_err(),
            "Existing ciphertext overwritten"
        );
        ensure!(
            Remote::new(&url, "wrong".repeat(9))?.get(id).await.is_err(),
            "Wrong credential accepted"
        );
        Ok(())
    }
}
