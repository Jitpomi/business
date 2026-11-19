//! Explicit checkout shadow migration: preserve every object and relationship.
use crate::database::{self, snapshot};
use anyhow::{ensure, Result};
use std::{collections::BTreeMap, path::Path};
use typedb_driver::TypeDBDriver;
pub async fn import(
    driver: &TypeDBDriver,
    verifier: &TypeDBDriver,
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
    verifier
        .databases()
        .import_from_file(&reference, old, &data)
        .await?;
    let original = snapshot::records(verifier, &reference).await?;
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
    let graph = snapshot::graph(verifier, &reference, &[]).await?;
    let approved_before = decisions(verifier, &reference).await?;

    let candidate = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    verifier
        .databases()
        .import_from_file(&candidate, current, &data)
        .await?;
    ensure!(
        snapshot::graph(verifier, &candidate, &[]).await? == graph,
        "Local migration changed original graph; source and shadows retained"
    );
    ensure!(
        decisions(verifier, &candidate).await? == approved_before,
        "Local migration changed existing plan approvals"
    );
    database::verify_runtime(verifier, &candidate).await?;
    let verified_schema = directory.join(format!("verified-{target}.tql"));
    let verified_data = directory.join(format!("verified-{target}.typedb"));
    ensure!(
        !verified_schema.exists() && !verified_data.exists(),
        "Verified migration export already exists; preserve this attempt"
    );
    verifier
        .databases()
        .get(&candidate)
        .await?
        .export_to_file(&verified_schema, &verified_data)
        .await?;
    // Only the fully verified candidate is imported on the application cluster.
    driver
        .databases()
        .import_from_file(
            target,
            std::fs::read_to_string(&verified_schema)?,
            verified_data,
        )
        .await?;
    ensure!(
        snapshot::graph(driver, target, &[]).await? == graph,
        "Checkout migration changed original graph; source and shadows retained"
    );
    ensure!(
        decisions(driver, target).await? == approved_before,
        "Migration changed existing plan approvals; source and shadows retained"
    );
    database::verify_runtime(driver, target).await?;
    // Historical consent grants remain byte-for-byte intact; no new approvals are manufactured.
    Ok(expected.len())
}

async fn decisions(driver: &TypeDBDriver, database: &str) -> Result<Vec<String>> {
    let tx = driver
        .transaction(database, typedb_driver::TransactionType::Read)
        .await?;
    let rows=database::documents(&tx,"match $c isa bill_customer;(customer:$c,plan:$p) isa bill_plan_owner;fetch {'customer':$c.bill_key,'plan':$p.bill_key,'approved':bill_plan_has_consent($c,$p),'choice':bill_plan_payment_choice($c,$p)};").await?;
    tx.close().await?;
    let mut normalized = rows
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    normalized.sort();
    Ok(normalized)
}
