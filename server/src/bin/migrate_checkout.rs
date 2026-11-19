//! Explicit development cutover. Rehearsal and preparation never write the source cluster.
use anyhow::{ensure, Result};
#[tokio::main]
async fn main() -> Result<()> {
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
    let verifier = server::recovery::local_driver().await?;
    let (target, n) = if args[1] == "--apply" {
        ensure!(
            std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
            "Quiesce all development writers first"
        );
        dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
        let driver = dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?;
        let expected: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(
                directory.join(format!("{source}.records.json")),
            )?)?;
        ensure!(
            server::database::snapshot::records(&driver, source).await? == expected,
            "Stale backup; source untouched"
        );
        let target = format!(
            "business_native_dev_v{}",
            server::database::development_version(source).unwrap() + 1
        );
        let n = server::commerce::migration::import(&driver, &verifier, source, &target, directory)
            .await?;
        (target, n)
    } else {
        let target = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
        let n =
            server::commerce::migration::import(&verifier, &verifier, source, &target, directory)
                .await?;
        (target, n)
    };
    println!("Checkout shadow: {target}");
    println!("Verified {n} original objects, attributes and relationship roles. Source and recovery shadows retained. No consent, provider effects or payment status invented.");
    Ok(())
}
