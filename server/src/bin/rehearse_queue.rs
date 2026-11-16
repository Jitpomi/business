//! Restore an explicit development backup and prove queue backfill preserves useful work.
use anyhow::{ensure, Result};
use server::{automation::store::Store, database::snapshot};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "Use rehearse_queue SOURCE PRIVATE_BACKUP_DIRECTORY"
    );
    let source = &args[1];
    ensure!(
        source == "business_native_dev_v3"
            || source
                .strip_prefix("business_backend_test_")
                .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok()),
        "Explicit development source required"
    );
    let directory = PathBuf::from(&args[2]);
    let expected: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.records.json")),
    )?)?;
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let target = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    driver
        .databases()
        .import_from_file(
            &target,
            std::fs::read_to_string(directory.join(format!("{source}.tql")))?,
            directory.join(format!("{source}.typedb")),
        )
        .await?;
    let original = snapshot::records(&driver, &target).await?;
    let values = |records: &BTreeMap<String, serde_json::Value>| -> Result<Vec<String>> {
        let mut values = records
            .values()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        values.sort();
        Ok(values)
    };
    ensure!(
        values(&original)? == values(&expected)?,
        "Backup restore differs; source untouched"
    );
    let graph = snapshot::graph(&driver, &target, &[]).await?;
    let state = Arc::new(server::typedb::TypeDBState {
        driver: driver.clone(),
        database: target.clone(),
    });
    let tx = driver
        .transaction(&target, typedb_driver::TransactionType::Read)
        .await?;
    let tenants =
        server::database::documents(&tx, "match $t isa tenant;fetch {'tenant':$t.tenant_id};")
            .await?;
    tx.close().await?;
    let mut inserted = 0;
    for tenant in tenants {
        let store = Store::new(
            state.clone(),
            tenant["tenant"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing tenant"))?,
        )?;
        inserted += server::background::reconcile(&store).await?;
        ensure!(
            server::background::reconcile(&store).await? == 0,
            "Backfill not idempotent"
        );
    }
    let after = snapshot::records(&driver, &target).await?;
    for (iid, old) in &original {
        let new = after
            .get(iid)
            .ok_or_else(|| anyhow::anyhow!("Original object missing"))?;
        ensure!(old == new, "Original business attributes changed");
    }
    let next_graph = snapshot::graph(&driver, &target, &[]).await?;
    ensure!(
        graph
            .iter()
            .all(|(key, count)| next_graph.get(key).is_some_and(|new| new >= count)),
        "Original graph or relation roles changed"
    );
    println!(
        "{}",
        serde_json::json!({"source":source,"shadow":target,"original_objects":original.len(),"added_deliveries":inserted,"preserved":true,"idempotent":true,"provider_calls":0})
    );
    Ok(())
}
