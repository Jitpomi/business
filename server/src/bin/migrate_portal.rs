//! Rehearse on a generated target; apply only with a fresh, quiesced backup.
use anyhow::{ensure, Result};
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3 && matches!(args[1].as_str(), "--rehearse" | "--apply"),
        "Use --rehearse/--apply and an explicit development source"
    );
    let source = &args[2];
    ensure!(
        matches!(source.as_str(), "business_native_dev" | "business_dev"),
        "Development only"
    );
    let directory = std::path::PathBuf::from(std::env::var("BUSINESS_BACKUP_DIR")?);
    for ext in ["tql", "typedb", "records.json"] {
        ensure!(
            directory.join(format!("{source}.{ext}")).is_file(),
            "Verified backup required"
        );
    }
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let target = if args[1] == "--apply" {
        ensure!(
            source == "business_native_dev"
                && std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
            "Quiesce development writers first"
        );
        let expected: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(
                directory.join(format!("{source}.records.json")),
            )?)?;
        ensure!(
            server::database::snapshot::records(&driver, source).await? == expected,
            "Backup is stale; export and restore-verify again"
        );
        "business_native_dev_v2".to_owned()
    } else {
        format!("business_backend_test_{}", uuid::Uuid::new_v4().simple())
    };
    println!("Shadow target: {target}");
    let preserved = server::portal_migration::import(&driver, source, &target, &directory).await?;
    println!("Preserved {preserved} original logical objects, business IDs and attributes. Source unchanged; target retained for acceptance checks. No provider calls or credential changes.");
    Ok(())
}
