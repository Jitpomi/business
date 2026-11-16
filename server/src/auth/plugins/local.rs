//! Optional single-host local-account plugin. Its durable store is outside TypeDB.
use anyhow::{ensure, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
pub struct Local {
    connection: Arc<Mutex<Connection>>,
}
impl Local {
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.is_absolute(),
            "Authentication store requires an absolute path"
        );
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing store directory"))?;
        ensure!(
            parent.is_dir(),
            "Create a private authentication directory explicitly"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            ensure!(
                std::fs::metadata(parent)?.permissions().mode() & 0o077 == 0,
                "Authentication directory must be private"
            );
            if !path.exists() {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)?;
            }
            ensure!(
                !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
                "Authentication store cannot be a symlink"
            );
            ensure!(
                std::fs::metadata(path)?.permissions().mode() & 0o077 == 0,
                "Authentication store must be private"
            );
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys=ON;PRAGMA journal_mode=DELETE;PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS accounts(subject TEXT PRIMARY KEY,login TEXT NOT NULL UNIQUE,hash TEXT NOT NULL,version TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS challenges(key TEXT PRIMARY KEY,payload TEXT NOT NULL,used INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS revoked(key TEXT PRIMARY KEY,expires INTEGER NOT NULL);")?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }
    pub fn configured(database: &str) -> Result<Self> {
        ensure!(
            database
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "Invalid auth namespace"
        );
        let root = std::path::PathBuf::from(std::env::var("AUTH_LOCAL_STORE_DIR")?);
        Self::open(&root.join(format!("{database}.sqlite3")))
    }
    pub fn lookup(&self, key: &str, by_login: bool) -> Result<Option<Value>> {
        let c = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?;
        let selector = if by_login { "login" } else { "subject" };
        Ok(c.query_row(&format!("SELECT subject,login,hash,version FROM accounts WHERE {selector}=?1"),[key],|r|Ok(json!({"subject":r.get::<_,String>(0)?,"email":r.get::<_,String>(1)?,"password":r.get::<_,String>(2)?,"version":r.get::<_,String>(3)?}))).optional()?)
    }
    pub fn import(&self, subject: &str, login: &str, hash: &str, version: &str) -> Result<()> {
        if let Some(old) = self.lookup(subject, false)? {
            ensure!(
                old["email"] == login && old["password"] == hash && old["version"] == version,
                "Existing authentication account differs; refusing overwrite"
            );
            return Ok(());
        }
        let c = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?;
        c.execute(
            "INSERT INTO accounts(subject,login,hash,version) VALUES(?1,?2,?3,?4)",
            params![subject, login, hash, version],
        )?;
        Ok(())
    }
    pub fn rotate(&self, subject: &str, hash: &str, version: &str) -> Result<()> {
        ensure!(
            self.connection
                .lock()
                .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?
                .execute(
                    "UPDATE accounts SET hash=?1,version=?2 WHERE subject=?3",
                    params![hash, version, subject]
                )?
                == 1,
            "Account unavailable"
        );
        Ok(())
    }
    pub fn remove(&self, subject: &str) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?
            .execute("DELETE FROM accounts WHERE subject=?1", [subject])?;
        Ok(())
    }
    pub fn recent(&self, kind: &str, email: &str, cutoff: i64) -> Result<bool> {
        Ok(self.connection.lock().map_err(|_|anyhow::anyhow!("Authentication store unavailable"))?.query_row("SELECT EXISTS(SELECT 1 FROM challenges WHERE json_extract(payload,'$.kind')=?1 AND json_extract(payload,'$.email')=?2 AND json_extract(payload,'$.expires')>?3)",params![kind,email,cutoff],|r|r.get(0))?)
    }
    pub fn challenge(&self, key: &str) -> Result<Option<Value>> {
        let c = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?;
        let v: Option<String> = c
            .query_row(
                "SELECT payload FROM challenges WHERE key=?1 AND used=0",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        v.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    }
    pub fn issue(&self, key: &str, payload: &Value) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?
            .execute(
                "INSERT INTO challenges(key,payload) VALUES(?1,?2)",
                params![key, payload.to_string()],
            )?;
        Ok(())
    }
    pub fn import_challenge(&self, key: &str, payload: &Value, used: bool) -> Result<()> {
        let c = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?;
        let old: Option<(String, bool)> = c
            .query_row(
                "SELECT payload,used FROM challenges WHERE key=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old, was_used)) = old {
            ensure!(
                serde_json::from_str::<Value>(&old)? == *payload && was_used == used,
                "Existing challenge differs; refusing overwrite"
            );
        } else {
            c.execute(
                "INSERT INTO challenges(key,payload,used) VALUES(?1,?2,?3)",
                params![key, payload.to_string(), used],
            )?;
        }
        Ok(())
    }
    pub fn consume(&self, key: &str) -> Result<bool> {
        Ok(self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?
            .execute(
                "UPDATE challenges SET used=1 WHERE key=?1 AND used=0",
                [key],
            )?
            == 1)
    }
    pub fn reset(
        &self,
        key: &str,
        subject: &str,
        version: &str,
        hash: &str,
        new_version: &str,
    ) -> Result<()> {
        let mut c = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            tx.execute(
                "UPDATE challenges SET used=1 WHERE key=?1 AND used=0",
                [key]
            )? == 1,
            "Recovery already used"
        );
        ensure!(
            tx.execute(
                "UPDATE accounts SET hash=?1,version=?2 WHERE subject=?3 AND version=?4",
                params![hash, new_version, subject, version]
            )? == 1,
            "Credentials changed"
        );
        tx.commit()?;
        Ok(())
    }
    pub fn revoked(&self, key: &str) -> Result<bool> {
        Ok(self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM revoked WHERE key=?1)",
                [key],
                |r| r.get(0),
            )?)
    }
    pub fn revoke(&self, key: &str, expires: i64) -> Result<bool> {
        Ok(self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Authentication store unavailable"))?
            .execute(
                "INSERT INTO revoked(key,expires) VALUES(?1,?2) ON CONFLICT(key) DO NOTHING",
                params![key, expires],
            )?
            == 1)
    }
}

impl crate::auth::accounts::LocalAccounts for Local {
    fn lookup(&self, key: &str, by_login: bool) -> Result<Option<Value>> {
        Local::lookup(self, key, by_login)
    }
    fn import(&self, subject: &str, login: &str, hash: &str, version: &str) -> Result<()> {
        Local::import(self, subject, login, hash, version)
    }
    fn challenge(&self, key: &str) -> Result<Option<Value>> {
        Local::challenge(self, key)
    }
    fn issue(&self, key: &str, payload: &Value) -> Result<()> {
        Local::issue(self, key, payload)
    }
    fn consume(&self, key: &str) -> Result<bool> {
        Local::consume(self, key)
    }
    fn reset(
        &self,
        key: &str,
        subject: &str,
        version: &str,
        hash: &str,
        new_version: &str,
    ) -> Result<()> {
        Local::reset(self, key, subject, version, hash, new_version)
    }
    fn recent(&self, kind: &str, email: &str, cutoff: i64) -> Result<bool> {
        Local::recent(self, kind, email, cutoff)
    }
    fn revoked(&self, key: &str) -> Result<bool> {
        Local::revoked(self, key)
    }
    fn revoke(&self, key: &str, expires: i64) -> Result<bool> {
        Local::revoke(self, key, expires)
    }
}
