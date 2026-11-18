//! Explicit development cutover; never rewrites the source or invokes providers.
use anyhow::{ensure, Result};
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4 && matches!(args[1].as_str(), "--rehearse" | "--apply"),
        "Use --rehearse/--apply SOURCE PRIVATE_CHECKPOINT_DIRECTORY"
    );
    let source = &args[2];
    let directory = std::path::Path::new(&args[3]);
    ensure!(
        server::database::development_version(source).is_some(),
        "Versioned development source only"
    );
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let target = if args[1] == "--apply" {
        ensure!(
            std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
            "Quiesce all development writers first"
        );
        let expected: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(
                directory.join(format!("{source}.records.json")),
            )?)?;
        ensure!(
            server::database::snapshot::records(&driver, source).await? == expected,
            "Stale backup; source untouched"
        );
        format!(
            "business_native_dev_v{}",
            server::database::development_version(source).unwrap() + 1
        )
    } else {
        format!("business_backend_test_{}", uuid::Uuid::new_v4().simple())
    };
    println!("Checkout shadow: {target}");
    let n = server::commerce::migration::import(&driver, source, &target, directory).await?;
    println!("Verified {n} original objects, attributes and relationship roles. Source and recovery shadows retained. No consent, provider effects or payment status invented.");
    Ok(())
}
