//! Run only with an explicit offline target and operator credentials.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::args().nth(1).as_deref() == Some("--apply"),
        "Review the migration guide; pass --apply with an explicit backed-up, quiesced target"
    );
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let name = std::env::var("TYPEDB_DB")?;
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let changed = server::plugin_migration::migrate(&driver, &name).await?;
    println!("Migrated {changed} customer binding maps; original references and billing records retained.");
    Ok(())
}
