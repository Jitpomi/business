//! Full recovery primitives. Restores only to a new generated database; never runs a worker or provider effect.
use aes_gcm::{
    aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm,
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};
const HEADER: &[u8] = b"JITPOMI-RECOVERY-v1\n";
const MAX: usize = 256 * 1024 * 1024;
fn private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
fn read(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        std::fs::metadata(path)?.len() <= MAX as u64,
        "Recovery file exceeds bound"
    );
    ensure!(
        !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
        "Recovery inputs cannot be symlinks"
    );
    Ok(std::fs::read(path)?)
}
pub fn encrypt(input: &Path, output: &Path, key_path: &Path) -> Result<()> {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow::anyhow!("Invalid recovery key"))?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let encrypted = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &read(input)?,
                aad: HEADER,
            },
        )
        .map_err(|_| anyhow::anyhow!("Recovery encryption failed"))?;
    let mut bytes = HEADER.to_vec();
    bytes.extend(nonce);
    bytes.extend(encrypted);
    // Key is stored separately, never inside its own encrypted archive.
    use base64::Engine;
    let encoded = format!(
        "JITPOMI-RECOVERY-KEY-V1\n{}\n",
        base64::engine::general_purpose::STANDARD.encode(key)
    );
    private_new(key_path, encoded.as_bytes())?;
    private_new(output, &bytes)
}
pub fn decrypt(input: &Path, output: &Path, key_path: &Path) -> Result<()> {
    let data = read(input)?;
    ensure!(
        data.starts_with(HEADER) && data.len() >= HEADER.len() + 28,
        "Invalid recovery envelope"
    );
    use base64::Engine;
    let encoded = String::from_utf8(read(key_path)?)?;
    let key = base64::engine::general_purpose::STANDARD.decode(
        encoded
            .strip_prefix("JITPOMI-RECOVERY-KEY-V1\n")
            .ok_or_else(|| anyhow::anyhow!("Invalid recovery key format"))?
            .trim(),
    )?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow::anyhow!("Invalid recovery key"))?;
    let payload = &data[HEADER.len()..];
    let plain = cipher
        .decrypt(
            aes_gcm::Nonce::from_slice(&payload[..12]),
            Payload {
                msg: &payload[12..],
                aad: HEADER,
            },
        )
        .map_err(|_| anyhow::anyhow!("Recovery archive authentication failed"))?;
    private_new(output, &plain)
}
pub async fn restore(
    driver: &typedb_driver::TypeDBDriver,
    directory: &Path,
    source: &str,
    target: &str,
) -> Result<()> {
    crate::database::target(source)?;
    uuid::Uuid::parse_str(
        target
            .strip_prefix("business_backend_test_")
            .ok_or_else(|| anyhow::anyhow!("Recovery requires a new generated shadow target"))?,
    )?;
    ensure!(
        source != target && !driver.databases().contains(target).await?,
        "Recovery target already exists or aliases source"
    );
    let expected: std::collections::BTreeMap<String, usize> =
        serde_json::from_slice(&read(&directory.join(format!("{source}.graph.json")))?)?;
    let schema = std::fs::read_to_string(directory.join(format!("{source}.tql")))?;
    driver
        .databases()
        .import_from_file(target, schema, directory.join(format!("{source}.typedb")))
        .await?;
    ensure!(
        crate::database::snapshot::graph(driver, target, &[]).await? == expected,
        "Restored graph differs; shadow retained for inspection"
    );
    crate::database::verify_runtime(driver, target).await?;
    Ok(())
}
/// Verify a D1 SQL export as a complete independent ciphertext store. No remote secret service is contacted.
pub async fn verify_vault(
    state: &crate::typedb::TypeDBState,
    sql: &Path,
    key: &str,
    canary: Option<&str>,
) -> Result<Value> {
    use crate::automation::{
        secret_store::{Memory, SecretStore},
        vault::Vault,
    };
    let db = rusqlite::Connection::open_in_memory()?;
    db.execute_batch(std::str::from_utf8(&read(sql)?)?)?;
    let mut statement = db.prepare("SELECT id,ciphertext FROM secrets ORDER BY id")?;
    let store = Arc::new(Memory::default());
    let mut total = 0;
    for row in statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, ciphertext) = row?;
        uuid::Uuid::parse_str(&id)?;
        store.put(&id, &ciphertext).await?;
        total += 1;
    }
    let vault = Vault::with_store(key, store.clone())?;
    use base64::Engine;
    let mut wrong_key = base64::engine::general_purpose::STANDARD.decode(key)?;
    ensure!(wrong_key.len() == 32, "Invalid key length");
    wrong_key[0] ^= 1;
    let wrong = Vault::with_store(
        &base64::engine::general_purpose::STANDARD.encode(wrong_key),
        store,
    )?;
    let tx = state
        .driver
        .transaction(&state.database, typedb_driver::TransactionType::Read)
        .await?;
    let rows=crate::database::documents(&tx,"match $r isa automation_record,has auto_kind 'connection';fetch {'tenant':$r.auto_tenant,'payload':$r.auto_payload};").await?;
    tx.close().await?;
    let mut verified = 0;
    for row in rows {
        let c: crate::automation::model::Connection = serde_json::from_str(
            row["payload"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid recovery record"))?,
        )?;
        if c.secret_ref.is_empty() {
            continue;
        }
        let context = format!(
            "{}/{}/{}",
            row["tenant"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid tenant context"))?,
            c.id,
            c.revision
        );
        ensure!(
            vault.open(&context, &c.secret_ref).await?.is_object(),
            "Invalid recovered secret"
        );
        ensure!(
            vault
                .open(&format!("foreign/{context}"), &c.secret_ref)
                .await
                .is_err()
                && wrong.open(&context, &c.secret_ref).await.is_err(),
            "Recovered vault accepted wrong context/key"
        );
        verified += 1;
    }
    if let Some(reference) = canary {
        ensure!(
            vault
                .open("business/operations/readiness/v1", reference)
                .await?
                == json!({"purpose":"operations-readiness","version":1}),
            "Restored operations canary invalid"
        );
    }
    ensure!(verified > 0, "No recovered encrypted connections");
    Ok(
        json!({"ciphertext_rows":total,"connection_credentials_verified":verified,"wrong_key_rejected":true,"wrong_tenant_rejected":true,"source_secret_store_contacted":false}),
    )
}
#[cfg(test)]
#[path = "../tests/unit/recovery.rs"]
mod tests;
