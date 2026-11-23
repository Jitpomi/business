//! Explicit checkout shadow migration: preserve every object and relationship.
use crate::database::{self, snapshot};
use anyhow::{ensure, Result};
use std::{collections::BTreeMap, path::Path};
use typedb_driver::TypeDBDriver;

/// Check portable facts and every relationship role before importing a successor.
/// Transactional backups are manifested from an isolated restore, whose internal
/// row IDs need not be the same as the source's storage IDs.
pub async fn verify_checkpoint_source(
    driver: &TypeDBDriver,
    source: &str,
    directory: &Path,
) -> Result<()> {
    let expected_records = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.records.json")),
    )?)?;
    let expected_graph = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.graph.json")),
    )?)?;
    let current_records = snapshot::records(driver, source).await?;
    let current_graph = snapshot::graph(driver, source, &[]).await?;
    verify_source_evidence(
        &expected_records,
        &current_records,
        &expected_graph,
        &current_graph,
    )
}

fn verify_source_evidence(
    expected_records: &BTreeMap<String, serde_json::Value>,
    current_records: &BTreeMap<String, serde_json::Value>,
    expected_graph: &BTreeMap<String, usize>,
    current_graph: &BTreeMap<String, usize>,
) -> Result<()> {
    let values = |records: &BTreeMap<String, serde_json::Value>| {
        let mut values = records
            .values()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>();
        values.sort();
        values
    };
    ensure!(
        values(expected_records) == values(current_records),
        "Source attributes differ from checkpoint; source untouched"
    );
    ensure!(
        expected_graph == current_graph,
        "Source relationship graph differs from checkpoint; source untouched"
    );
    Ok(())
}

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
    let expected_graph: BTreeMap<String, usize> = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.graph.json")),
    )?)?;
    ensure!(
        graph == expected_graph,
        "Checkpoint relationship manifest differs from restored source; all databases retained"
    );
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

/// Rehearse the complete checkpoint locally, then update the existing development
/// resource in one exclusive schema transaction. No original data is rewritten.
pub async fn apply_schema(
    driver: &TypeDBDriver,
    verifier: &TypeDBDriver,
    source: &str,
    directory: &Path,
) -> Result<serde_json::Value> {
    ensure!(
        database::development_version(source).is_some(),
        "Development source required"
    );
    ensure!(
        std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
        "Quiesce all development writers first"
    );
    let shadow = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    let count = import(verifier, verifier, source, &shadow, directory).await?;
    let expected_schema = std::fs::read_to_string(directory.join(format!("{source}.tql")))?;
    let target_schema = verifier.databases().get(&shadow).await?.schema().await?;
    let plan = database::schema_upgrade::plan(&expected_schema, &target_schema)?;
    let attempt = directory.join(format!(
        "schema-update-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    let intent = serde_json::json!({"database":source,"local_rehearsal":shadow,"records":count,"plan":plan,"committed":false});
    crate::recovery::private_new(&attempt, &serde_json::to_vec_pretty(&intent)?)?;
    // The schema transaction excludes every other schema/write transaction.
    // Concurrent reads remain available for full checkpoint verification.
    let tx = driver
        .transaction(source, typedb_driver::TransactionType::Schema)
        .await?;
    let checked: Result<_> = async {
        let actual = database::schema_upgrade::fingerprint(
            &driver.databases().get(source).await?.schema().await?,
        )?;
        ensure!(
            actual == plan.source_sha256 || actual == plan.target_sha256,
            "Source schema differs from the rehearsed checkpoint; nothing changed"
        );
        verify_checkpoint_source(driver, source, directory).await?;
        let approved = decisions(driver, source).await?;
        if actual != plan.target_sha256 {
            if !plan.define.is_empty() {
                tx.query(&plan.define).await?;
            }
            for query in &plan.redefine {
                tx.query(query).await?;
            }
        }
        Ok(approved)
    }
    .await;
    let approved = match checked {
        Ok(approved) => approved,
        Err(error) => {
            tx.close().await?;
            return Err(error);
        }
    };
    // On an interrupted acknowledgement, a rerun accepts only the exact target
    // schema and unchanged checkpoint, without replaying any provider action.
    tx.commit().await?;
    ensure!(
        database::schema_upgrade::fingerprint(
            &driver.databases().get(source).await?.schema().await?
        )? == plan.target_sha256,
        "Committed schema differs; retain checkpoint and investigate"
    );
    verify_checkpoint_source(driver, source, directory).await?;
    ensure!(
        decisions(driver, source).await? == approved,
        "Existing approvals changed; retain checkpoint and investigate"
    );
    database::verify_runtime(driver, source).await?;
    let report = serde_json::json!({"database":source,"records":count,"committed":true,
        "checkpoint_preserved":true,"approvals_preserved":true,"runtime_verified":true,
        "target_sha256":plan.target_sha256,"local_rehearsal":shadow});
    crate::recovery::private_new(
        &attempt.with_extension("complete.json"),
        &serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
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
/// Fence application access in a retired development generation only after its
/// successor exactly matches the checkpoint. This is an explicit cutover step,
/// never startup behaviour or a payment-state mutation.
pub async fn retire_source(
    driver: &TypeDBDriver,
    source: &str,
    target: &str,
    directory: &Path,
) -> Result<serde_json::Value> {
    use serde_json::{json, Value};
    let version = database::development_version(source)
        .ok_or_else(|| anyhow::anyhow!("Versioned development source required"))?;
    ensure!(
        database::development_version(target) == Some(version + 1),
        "Explicit next development generation required"
    );
    database::verify_runtime(driver, target).await?;
    let original: BTreeMap<String, Value> = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.records.json")),
    )?)?;
    let expected_graph: BTreeMap<String, usize> = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.graph.json")),
    )?)?;
    ensure!(
        snapshot::graph(driver, target, &[]).await? == expected_graph,
        "Successor changed since verification; retire nothing"
    );
    let normalize = |rows: &BTreeMap<String, Value>| -> Result<Vec<String>> {
        let mut values = rows
            .values()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        values.sort();
        Ok(values)
    };
    let mut fenced = original.clone();
    let mut tenants = Vec::new();
    for row in fenced.values_mut() {
        if row.get("tenant_id").is_some() && row["status"] == "active" {
            tenants.push(row["tenant_id"].clone());
            row["status"] = json!("suspended");
        }
    }
    ensure!(
        !tenants.is_empty(),
        "Checkpoint contains no active development tenants"
    );
    tenants.sort_by_key(Value::to_string);
    let current = snapshot::records(driver, source).await?;
    let journal = directory.join(format!("retire-{source}.intent.json"));
    let without_status = snapshot::graph(driver, source, &["status"]).await?;
    let intent = if journal.exists() {
        let value: Value = serde_json::from_slice(&std::fs::read(&journal)?)?;
        ensure!(
            value["source"] == source
                && value["target"] == target
                && value["tenants"] == json!(tenants)
                && value["graph_without_status"] == json!(without_status),
            "Retirement intent differs; preserve it and stop"
        );
        value
    } else {
        ensure!(
            normalize(&current)? == normalize(&original)?
                && snapshot::graph(driver, source, &[]).await? == expected_graph,
            "Source changed; retire nothing"
        );
        let value = json!({"source":source,"target":target,"tenants":tenants,"graph_without_status":without_status});
        crate::recovery::private_new(&journal, &serde_json::to_vec_pretty(&value)?)?;
        value
    };
    if normalize(&current)? != normalize(&fenced)? {
        ensure!(
            normalize(&current)? == normalize(&original)?
                && snapshot::graph(driver, source, &[]).await? == expected_graph,
            "Source changed; no retirement write attempted"
        );
        let tx = driver
            .transaction(source, typedb_driver::TransactionType::Write)
            .await?;
        let rows=database::documents(&tx,"match $t isa tenant,has status 'active';update $t has status 'suspended';fetch {'id':$t.tenant_id};").await?;
        ensure!(
            rows.len() == tenants.len(),
            "Active tenant set changed; transaction not committed"
        );
        tx.commit().await?;
    }
    // Records independently check every status; the graph comparison additionally
    // checks all relationship roles without treating the intentional fence as loss.
    ensure!(
        normalize(&snapshot::records(driver, source).await?)? == normalize(&fenced)?
            && json!(snapshot::graph(driver, source, &["status"]).await?)
                == intent["graph_without_status"],
        "Retired source differs beyond tenant status; retain evidence"
    );
    ensure!(
        snapshot::graph(driver, target, &[]).await? == expected_graph,
        "Successor changed during retirement; retain evidence"
    );
    let report = json!({"source":source,"target":target,"retired_tenants":tenants.len(),"only_retired_tenant_status_changed":true,"all_relationship_roles_preserved":true,"successor_preserved_original_graph":true,"payment_state_overrides":false,"provider_calls":false});
    let proof = directory.join(format!("retire-{source}.complete.json"));
    if !proof.exists() {
        crate::recovery::private_new(&proof, &serde_json::to_vec_pretty(&report)?)?;
    }
    Ok(report)
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn portable_ids_do_not_hide_attribute_or_relationship_changes() {
        let record = json!({"bill_key":"invoice", "bill_amount":100, "bill_status":"paid"});
        let original = BTreeMap::from([("source-storage-id".into(), record.clone())]);
        let restored = BTreeMap::from([("restored-storage-id".into(), record)]);
        let graph = BTreeMap::from([("tenant-owned-invoice-graph-digest".into(), 1)]);
        assert!(verify_source_evidence(&original, &restored, &graph, &graph).is_ok());

        let mut altered = restored.clone();
        altered.get_mut("restored-storage-id").unwrap()["bill_amount"] = json!(200);
        assert!(verify_source_evidence(&original, &altered, &graph, &graph).is_err());
        assert!(verify_source_evidence(&original, &BTreeMap::new(), &graph, &graph).is_err());

        // The same entity attributes can be connected to a different tenant.
        let foreign_graph = BTreeMap::from([("different-tenant-owned-invoice-digest".into(), 1)]);
        assert!(verify_source_evidence(&original, &restored, &graph, &foreign_graph).is_err());
        let duplicate_graph = BTreeMap::from([("tenant-owned-invoice-graph-digest".into(), 2)]);
        assert!(verify_source_evidence(&original, &restored, &graph, &duplicate_graph).is_err());
    }
}
