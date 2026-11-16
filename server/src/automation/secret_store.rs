//! External secret persistence. TypeDB receives only opaque references.
use anyhow::{ensure, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub trait SecretStore: Send + Sync {
    fn put(&self, id: &str, ciphertext: &str) -> Result<()>;
    fn get(&self, id: &str) -> Result<String>;
}
#[derive(Default)]
pub struct Memory(Mutex<HashMap<String, String>>);
impl SecretStore for Memory {
    fn put(&self, id: &str, value: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Secret store unavailable"))?
            .insert(id.into(), value.into());
        Ok(())
    }
    fn get(&self, id: &str) -> Result<String> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Secret store unavailable"))?
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Secret reference unavailable"))
    }
}
pub struct Files(PathBuf);
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
impl SecretStore for Files {
    fn put(&self, id: &str, value: &str) -> Result<()> {
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
    fn get(&self, id: &str) -> Result<String> {
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
