//! Preserve the entire graph in a new development shadow; never infer buyer consent.
use crate::database::{self, documents, snapshot};
use anyhow::{ensure, Result};
use std::{collections::BTreeMap, path::Path};
use typedb_driver::{TransactionType, TypeDBDriver};

pub async fn import(
    driver: &TypeDBDriver,
    source: &str,
    target: &str,
    directory: &Path,
) -> Result<usize> {
    database::target(source)?;
    database::target(target)?;
    ensure!(
        source != "business_prod" && target != "business_prod" && source != target,
        "Development shadows only"
    );
    ensure!(
        target == "business_native_dev_v3" || target.starts_with("business_backend_test_"),
        "New consent shadow required"
    );
    ensure!(
        !driver.databases().contains(target).await?,
        "Target exists; nothing overwritten"
    );
    let old = std::fs::read_to_string(directory.join(format!("{source}.tql")))?;
    let current = database::import_schema();
    let unknown: Vec<_> = crate::portal_migration::definitions(&old)?
        .difference(&crate::portal_migration::definitions(&current)?)
        .filter(|name| {
            ![
                "function:update_bill_choice",
                "function:update_bill_consent",
            ]
            .contains(&name.as_str())
        })
        .cloned()
        .collect();
    ensure!(
        unknown.is_empty(),
        "Custom schema needs preservation review before cutover: {unknown:?}"
    );
    let data = directory.join(format!("{source}.typedb"));
    let expected: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.records.json")),
    )?)?;
    // Importing first into the exact exported schema proves the backup, including
    // attributes and relation role bindings. Both original and failed shadows survive.
    let native_source = old.contains("fun bill_has_consent");
    let plan_source = old.contains("fun bill_plan_has_consent");
    let reference = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    driver
        .databases()
        .import_from_file(&reference, old, &data)
        .await?;
    let restored = snapshot::records(driver, &reference).await?;
    let values = |m: &BTreeMap<String, serde_json::Value>| {
        let mut v: Vec<_> = m
            .values()
            .map(serde_json::to_string)
            .collect::<std::result::Result<_, _>>()?;
        v.sort();
        Ok::<_, anyhow::Error>(v)
    };
    ensure!(
        values(&restored)? == values(&expected)?,
        "Backup records mismatch; source untouched"
    );
    let before = snapshot::graph(driver, &reference, &[]).await?;
    let approved_before = if native_source {
        let tx = driver
            .transaction(&reference, TransactionType::Read)
            .await?;
        let policy = if plan_source {
            "bill_plan_has_consent($c,$p)"
        } else {
            "bill_has_consent($c)"
        };
        let rows=documents(&tx,&format!("match $c isa bill_customer;$p isa bill_plan;(customer:$c,plan:$p) isa bill_plan_owner;let $approved={policy};$approved==true;fetch {{'customer':$c.bill_key,'plan':$p.bill_key}};")).await?;
        tx.close().await?;
        rows
    } else {
        Vec::new()
    };

    driver
        .databases()
        .import_from_file(target, database::import_schema(), &data)
        .await?;
    ensure!(
        snapshot::graph(driver, target, &[]).await? == before,
        "Shadow changed original objects, attributes or relation roles"
    );
    let tx = driver.transaction(target, TransactionType::Read).await?;
    let active=documents(&tx,"match $c isa bill_customer,has bill_key $id,has bill_choice 'automatic',has bill_consent $rev,has bill_revision == $rev;$rev!='';let $valid=bill_has_consent($c);$valid==false;fetch {'customer':$id};").await?;
    let approved_after=documents(&tx,"match $c isa bill_customer;$p isa bill_plan;(customer:$c,plan:$p) isa bill_plan_owner;let $approved=bill_plan_has_consent($c,$p);$approved==true;fetch {'customer':$c.bill_key,'plan':$p.bill_key};").await?;
    tx.close().await?;
    ensure!(active.is_empty(),"Legacy active consent has no verifiable native grant. Original records preserved; do not cut over. Obtain buyer re-consent before migrating these customers: {active:?}");
    ensure!(approved_before.len()==approved_after.len()&&approved_before.iter().all(|row|approved_after.contains(row)),"Source and shadow authorized-plan sets differ. All original records preserved; review explicit buyer plan scope before cutover");
    crate::business_rules::backfill(driver, target).await?;
    database::verify_runtime(driver, target).await?;
    driver.databases().get(&reference).await?.delete().await?;
    Ok(expected.len())
}
