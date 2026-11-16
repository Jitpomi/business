//! Explicit backup-verified development cutover; no startup migrations or provider calls.
use anyhow::{ensure, Result};
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3 && matches!(args[1].as_str(), "--rehearse" | "--apply" | "--inspect"),
        "Use --rehearse/--apply and explicit source"
    );
    let source = &args[2];
    ensure!(
        source == "business_native_dev_v2" || source.starts_with("business_backend_test_"),
        "Development source only"
    );
    let dir = std::env::var("BUSINESS_BACKUP_DIR")
        .map(std::path::PathBuf::from)
        .ok();
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    if args[1] == "--inspect" {
        let tx = driver
            .transaction(source, typedb_driver::TransactionType::Read)
            .await?;
        let customers=server::database::documents(&tx,"match $c isa bill_customer;fetch {'id':$c.bill_key,'revision':$c.bill_revision,'legacy_choice':$c.bill_choice,'legacy_consent':$c.bill_consent};").await?;
        let grants=server::database::documents(&tx,"match $e isa bill_consent_evidence;fetch {'id':$e.bill_key,'revision':$e.bill_revision,'native_claim':[$e.consent_claim]};").await?;
        println!(
            "{}",
            serde_json::json!({"customers":customers,"grants":grants})
        );
        return Ok(());
    }
    let dir = dir.ok_or_else(|| anyhow::anyhow!("Private verified backup directory required"))?;
    let target = if args[1] == "--apply" {
        ensure!(
            source == "business_native_dev_v2"
                && std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
            "Stop writers and verify a fresh backup first"
        );
        let expected: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.join(format!("{source}.records.json")))?)?;
        ensure!(
            server::database::snapshot::records(&driver, source).await? == expected,
            "Backup is stale; do not cut over"
        );
        "business_native_dev_v3".to_owned()
    } else {
        format!("business_backend_test_{}", uuid::Uuid::new_v4().simple())
    };
    println!("Consent shadow: {target}");
    let n = server::consent_migration::import(&driver, source, &target, &dir).await?;
    println!("Verified preservation of {n} original objects and relation roles. Source and backup untouched; target retained. No provider calls or consent manufactured.");
    Ok(())
}
