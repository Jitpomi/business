//! Explicit development shadow migration; never modifies an existing database.
use anyhow::{ensure, Result};
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4 && matches!(args[1].as_str(), "--rehearse" | "--apply"),
        "Use --rehearse/--apply SOURCE PRIVATE_BACKUP_DIRECTORY"
    );
    let source = &args[2];
    let directory = std::path::Path::new(&args[3]);
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let target = if args[1] == "--apply" {
        ensure!(
            source == "business_native_dev_v3"
                && std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
            "Quiesce development writers before cutover"
        );
        let expected: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(
                directory.join(format!("{source}.records.json")),
            )?)?;
        ensure!(
            server::database::snapshot::records(&driver, source).await? == expected,
            "Backup stale; source untouched"
        );
        "business_native_dev_v4".into()
    } else {
        format!("business_backend_test_{}", uuid::Uuid::new_v4().simple())
    };
    println!("Native policy shadow: {target}");
    println!(
        "{}",
        server::business_rules::import(&driver, source, &target, directory).await?
    );
    Ok(())
}
