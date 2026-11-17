//! Explicit native-policy projection migration. Original payloads, claims, revisions,
//! timestamps and relationship roles are preserved. No provider calls or startup writes.
use crate::{
    automation::{policy, store::q},
    database::{self, documents, snapshot},
};
use anyhow::{ensure, Result};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};
use typedb_driver::{TransactionType, TypeDBDriver};

pub async fn backfill(driver: &TypeDBDriver, target: &str) -> Result<usize> {
    database::target(target)?;
    ensure!(
        target != "business_prod",
        "Explicit development shadow required"
    );
    let before = snapshot::graph(driver, target, policy::ATTRIBUTES).await?;
    let mut after = String::new();
    let mut changed = 0;
    loop {
        let tx = driver.transaction(target, TransactionType::Read).await?;
        let rows=documents(&tx,&format!("match $r isa automation_record,has auto_key $key;$key > {};select $r,$key;sort $key;limit 25;fetch {{'key':$r.auto_key,'facts':{{$r.*}}}};",q(&after))).await?;
        tx.close().await?;
        if rows.is_empty() {
            break;
        }
        let mut patterns = String::new();
        let mut updates = String::new();
        let mut projection = Vec::new();
        for row in &rows {
            let key = row["key"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Record key missing"))?;
            after = key.into();
            let original = &row["facts"];
            let payload = original["auto_payload"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Record payload missing"))?;
            let value: Value = serde_json::from_str(payload)?;
            let facts = policy::facts(
                original["auto_kind"].as_str().unwrap_or(""),
                &value,
                original["auto_status"].as_str().unwrap_or(""),
            )?;
            let mut missing = false;
            for (name, expected) in &facts {
                match original.get(name) {
                    Some(current)=>ensure!(current==expected,"Native facts disagree with preserved payload: {key}; shadow retained for review"),
                    None=>missing=true,
                }
            }
            if !missing {
                continue;
            }
            let index = projection.len();
            let variable = format!("policy_{index}");
            patterns.push_str(&format!("${variable} isa automation_record,has auto_key {},has auto_revision {},has auto_payload {};",q(key),original["auto_revision"],q(payload)));
            let attributes = facts
                .iter()
                .map(|(name, value)| format!("has {name} {value}"))
                .collect::<Vec<_>>()
                .join(",");
            updates.push_str(&format!("${variable} {attributes};"));
            projection.push((
                key.to_owned(),
                format!("'row_{index}':${variable}.auto_key"),
            ));
        }
        if !projection.is_empty() {
            let tx = driver.transaction(target, TransactionType::Write).await?;
            let query = format!(
                "match {patterns} update {updates} fetch {{{}}};",
                projection
                    .iter()
                    .map(|(_, field)| field.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let updated = documents(&tx, &query).await?;
            ensure!(
                updated.len() == 1
                    && projection
                        .iter()
                        .enumerate()
                        .all(|(i, (key, _))| updated[0][format!("row_{i}")] == key.as_str()),
                "Record changed during bounded policy batch; shadow retained"
            );
            // Commit only policy facts, once. Never retry a commit or a provider action.
            tx.commit().await?;
            changed += projection.len();
        }
        if changed > 0 && changed % 100 == 0 {
            eprintln!("Projected {changed} records; original payloads retained");
        }
    }
    ensure!(
        snapshot::graph(driver, target, policy::ATTRIBUTES).await? == before,
        "Original graph changed during native projection; source and shadow retained"
    );
    verify(driver, target).await?;
    Ok(changed)
}

pub async fn verify(driver: &TypeDBDriver, target: &str) -> Result<()> {
    let schema = driver.databases().get(target).await?.schema().await?;
    ensure!(
        schema.contains("fun automation_step_progress")
            && schema.contains("fun bill_notice_decision")
            && schema.contains("fun bill_next_occurrence")
            && schema.contains("fun automation_oauth_claim")
            && schema.contains("fun automation_oauth_finish"),
        "Explicit native business-rule migration required before runtime"
    );
    let tx = driver.transaction(target, TransactionType::Read).await?;
    let rows=documents(&tx,r#"match $r isa automation_record,has auto_kind $kind;
      {$kind=="run";not {$r has auto_owner $_,has auto_cursor $_,has auto_steps $_,has auto_attempts $_,has auto_effect $_,has auto_definition $_,has auto_step $_,has auto_approved $_,has auto_approval_history $_;};}
      or {{$kind=="workflow";} or {$kind=="workflow_version";};not {$r has auto_owner $_,has auto_enabled $_;};}
      or {$kind=="oauth";not {$r has auto_owner $_,has auto_definition $_;};}
      or {$kind=="connection";not {$r has auto_enabled $_,has auto_verified $_,has auto_binding_revision $_;};}
      or {{$kind=="schedule";} or {$kind=="trigger";};not {$r has auto_enabled $_,has auto_definition $_;};};
      reduce $n=count;fetch {'missing':$n};"#).await?;
    tx.close().await?;
    ensure!(
        rows.len() == 1 && crate::billing::engine::number(&rows[0], "missing")? == 0,
        "Native policy facts are incomplete; run the explicit verified shadow migration"
    );
    Ok(())
}

pub async fn import(
    driver: &TypeDBDriver,
    source: &str,
    target: &str,
    directory: &Path,
) -> Result<Value> {
    database::target(source)?;
    database::target(target)?;
    ensure!(
        (database::development_version(source).is_some()
            || source.starts_with("business_backend_test_"))
            && (database::development_version(target).is_some()
                || target.starts_with("business_backend_test_"))
            && source != target,
        "Development source and new policy shadow required"
    );
    if let (Some(from), Some(to)) = (
        database::development_version(source),
        database::development_version(target),
    ) {
        ensure!(
            to > from,
            "A policy shadow must advance the development version"
        );
    }
    ensure!(
        !driver.databases().contains(target).await?,
        "Target exists; nothing overwritten"
    );
    let old = std::fs::read_to_string(directory.join(format!("{source}.tql")))?;
    let current = upgrade_schema(&old)?;
    let data = directory.join(format!("{source}.typedb"));
    let expected: BTreeMap<String, Value> = serde_json::from_slice(&std::fs::read(
        directory.join(format!("{source}.records.json")),
    )?)?;
    let reference = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Restoring immutable backup reference: {reference}");
    driver
        .databases()
        .import_from_file(&reference, old, &data)
        .await?;
    eprintln!("Checking restored backup records");
    let restored = snapshot::records(driver, &reference).await?;
    fn values(m: &BTreeMap<String, Value>) -> Result<Vec<String>> {
        let mut v = m
            .values()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        v.sort();
        Ok(v)
    }
    ensure!(
        values(&expected)? == values(&restored)?,
        "Backup mismatch; all databases retained; source untouched"
    );
    eprintln!("Checking restored relationship graph");
    let before = snapshot::graph(driver, &reference, &[]).await?;
    eprintln!("Importing additive policy shadow: {target}");
    driver
        .databases()
        .import_from_file(target, current, &data)
        .await?;
    ensure!(
        snapshot::graph(driver, target, &[]).await? == before,
        "Canonical import changed original graph; source and backup untouched"
    );
    eprintln!("Backfilling policy facts; original payloads and journals remain unchanged");
    let projected = backfill(driver, target).await?;
    ensure!(
        backfill(driver, target).await? == 0,
        "Policy backfill is not idempotent"
    );
    database::verify_runtime(driver, target).await?;
    // Keep both verification and cutover shadows as evidence, never delete useful work.
    Ok(
        serde_json::json!({"source":source,"target":target,"reference":reference,"original_objects":expected.len(),"projected_records":projected,"preserved":true,"idempotent":true,"provider_calls":0}),
    )
}

/// Extend the exact exported schema. No original type, constraint or function is
/// replaced, including custom tenant work. Colliding policy functions need review.
fn upgrade_schema(old: &str) -> Result<String> {
    use typeql::{
        query::{schema::SchemaQuery, QueryStructure},
        schema::definable::Definable,
    };
    let parse = |source: &str| -> Result<Vec<Definable>> {
        let mut definitions = Vec::new();
        for q in typeql::parse_queries(source)? {
            let QueryStructure::Schema(SchemaQuery::Define(d)) = q.structure else {
                anyhow::bail!("Unexpected schema source")
            };
            definitions.extend(d.definables);
        }
        Ok(definitions)
    };
    let old_definitions = parse(old)?;
    let functions = old_definitions
        .iter()
        .filter_map(|d| {
            if let Definable::Function(f) = d {
                Some((f.signature.ident.to_string(), f.to_string()))
            } else {
                None
            }
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let types = crate::portal_migration::definitions(old)?;
    let mut extra = String::new();
    for d in parse(crate::automation::store::POLICY_SCHEMA)? {
        if let Definable::TypeDeclaration(t) = &d {
            if t.label.to_string() != "automation_record"
                && types.contains(&format!("type:{}", t.label))
            {
                continue;
            }
        }
        extra.push_str(&d.to_string());
        extra.push('\n');
    }
    for d in parse(database::BUSINESS_RULES)? {
        if let Definable::Function(f) = &d {
            if let Some(original) = functions.get(&f.signature.ident.to_string()) {
                ensure!(
                    original == &f.to_string(),
                    "Existing policy function {} differs; review preserved source schema",
                    f.signature.ident
                );
                continue;
            }
        }
        extra.push_str(&d.to_string());
        extra.push('\n');
    }
    let preserved = old_definitions
        .iter()
        .map(|d| format!("{d}\n"))
        .collect::<String>();
    Ok(format!("define\n{preserved}{extra}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_preserves_custom_schema_and_refuses_policy_collisions() -> Result<()> {
        let original = format!("{}\ndefine entity customer_note, owns name;\nfun customer_note_exists() -> boolean:\n match $n isa customer_note; return check;", crate::automation::store::SCHEMA);
        let upgraded = upgrade_schema(&original)?;
        let definitions = crate::portal_migration::definitions(&upgraded)?;
        for definition in crate::portal_migration::definitions(&original)? {
            ensure!(
                definitions.contains(&definition),
                "Original definition lost"
            );
        }
        ensure!(upgraded.contains("customer_note_exists"));
        let conflicting = format!("{original}\ndefine fun bill_valid_reminder_days($days: integer) -> boolean:\n match $days >= 0; return check;");
        ensure!(upgrade_schema(&conflicting).is_err());
        Ok(())
    }
}
