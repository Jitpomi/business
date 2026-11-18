//! Explicit checkout shadow migration: preserve every object and relationship.
use crate::database::{self, snapshot};
use anyhow::{ensure, Result};
use std::{collections::BTreeMap, path::Path};
use typedb_driver::TypeDBDriver;
pub async fn import(
    driver: &TypeDBDriver,
    source: &str,
    target: &str,
    directory: &Path,
) -> Result<usize> {
    ensure!(
        database::development_version(source).is_some()
            || source.starts_with("business_backend_test_"),
        "Development source required"
    );
    database::target(target)?;
    ensure!(
        target != "business_prod"
            && target != source
            && !driver.databases().contains(target).await?,
        "New development shadow required"
    );
    let old = std::fs::read_to_string(directory.join(format!("{source}.tql")))?;
    let current = database::import_schema();
    let unknown: Vec<_> = crate::portal_migration::definitions(&old)?
        .difference(&crate::portal_migration::definitions(&current)?)
        .cloned()
        .collect();
    ensure!(
        unknown.is_empty(),
        "Preserve and review custom definitions before migration: {unknown:?}"
    );
    let expected: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.records.json")),
    )?)?;
    let reference = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    let data = directory.join(format!("{source}.typedb"));
    driver
        .databases()
        .import_from_file(&reference, old, &data)
        .await?;
    let original = snapshot::records(driver, &reference).await?;
    let values = |records: &BTreeMap<String, serde_json::Value>| -> Result<Vec<String>> {
        let mut v = records
            .values()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        v.sort();
        Ok(v)
    };
    ensure!(
        values(&expected)? == values(&original)?,
        "Checkpoint does not match restored source; all databases retained"
    );
    let graph = snapshot::graph(driver, &reference, &[]).await?;
    let decisions = |database: String| async move {
        let tx = driver
            .transaction(database, typedb_driver::TransactionType::Read)
            .await?;
        let rows = database::documents(&tx,"match $c isa bill_customer;(customer:$c,plan:$p) isa bill_plan_owner;fetch {'customer':$c.bill_key,'plan':$p.bill_key,'approved':bill_plan_has_consent($c,$p),'choice':bill_plan_payment_choice($c,$p)};").await?;
        tx.close().await?;
        let mut normalized = rows
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        normalized.sort();
        Ok::<_, anyhow::Error>(normalized)
    };
    let approved_before = decisions(reference.clone()).await?;

    driver
        .databases()
        .import_from_file(target, current, &data)
        .await?;
    ensure!(
        snapshot::graph(driver, target, &[]).await? == graph,
        "Checkout migration changed original graph; source and shadows retained"
    );
    ensure!(
        decisions(target.to_owned()).await? == approved_before,
        "Migration changed existing plan approvals; source and shadows retained"
    );
    database::verify_runtime(driver, target).await?;
    // Historical consent grants remain byte-for-byte intact; no new approvals are manufactured.
    Ok(expected.len())
}
