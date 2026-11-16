//! Explicit offline migration; never invoked by application startup.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::args().nth(1).as_deref() == Some("--apply"),
        "Pass --apply only after a verified backup and stopping all writers"
    );
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    anyhow::ensure!(
        std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
        "Stop all writers first"
    );
    let directory = std::path::PathBuf::from(std::env::var("BUSINESS_BACKUP_DIR")?);
    let name = std::env::var("TYPEDB_DB")?;
    server::database::target(&name)?;
    for extension in ["tql", "typedb", "records.json"] {
        anyhow::ensure!(
            directory.join(format!("{name}.{extension}")).is_file(),
            "Missing pre-migration backup artifact"
        );
    }
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let before: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(directory.join(format!("{name}.records.json")))?,
    )?;
    anyhow::ensure!(
        before == server::database::snapshot::records(&driver, &name).await?,
        "Backup is stale; export and restore-verify a fresh backup before applying"
    );
    let count = server::credential_migration::migrate(&driver, &name).await?;
    println!("Evacuated {count} authentication accounts to the external plugin store; identity and business history preserved. Configure and verify provider sessions before reopening access.");
    Ok(())
}
