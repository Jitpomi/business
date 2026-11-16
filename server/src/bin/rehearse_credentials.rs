//! Rehearse credential evacuation on a restored, generated database only.
use anyhow::{ensure, Result};
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let source = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Explicit development source required"))?;
    ensure!(
        matches!(source.as_str(), "business_dev" | "business_native_dev"),
        "Development source only"
    );
    let root = std::path::PathBuf::from(std::env::var("BUSINESS_BACKUP_DIR")?);
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    driver
        .databases()
        .import_from_file(
            &db,
            std::fs::read_to_string(root.join(format!("{source}.tql")))?,
            root.join(format!("{source}.typedb")),
        )
        .await?;
    // Keep failed rehearsals for inspection. Never change or replace the source database.
    println!("Rehearsal database: {db}");
    let before = server::database::snapshot::records(&driver, &db).await?;
    let accounts = server::credential_migration::migrate(&driver, &db).await?;
    let after = server::database::snapshot::records(&driver, &db).await?;
    let mut preserved = 0;
    for (iid, old) in &before {
        if old.get("token_key").is_some() || old.get("credential_id").is_some() {
            continue;
        }
        let current = after
            .get(iid)
            .ok_or_else(|| anyhow::anyhow!("Existing business record missing"))?;
        for (key, value) in old.as_object().unwrap() {
            if matches!(key.as_str(), "password_hash" | "credential_version") {
                continue;
            }
            let connection = old.get("auto_kind").is_some_and(|v| v == "connection");
            if connection && key == "auto_revision" {
                continue;
            }
            if connection && key == "auto_payload" {
                let old_payload = value
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid payload snapshot"))?;
                let new_payload = current[key]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid payload snapshot"))?;
                let mut old: serde_json::Value = serde_json::from_str(old_payload)?;
                let mut new: serde_json::Value = serde_json::from_str(new_payload)?;
                old.as_object_mut().unwrap().remove("sealed");
                new.as_object_mut().unwrap().remove("secret_ref");
                ensure!(old == new, "Connection configuration changed");
            } else {
                ensure!(&current[key] == value, "Business attribute changed: {key}");
            }
        }
        preserved += 1;
    }
    ensure!(
        server::credential_migration::migrate(&driver, &db).await? == 0,
        "Migration not repeatable"
    );
    println!("Rehearsal passed: {accounts} external accounts, {preserved} existing business records preserved; schema verified; repeated migration passed");
    driver.databases().get(&db).await?.delete().await?;
    Ok(())
}
