//! Shadow import preserves the source and every original object/attribute. No
//! source schema mutation, reset, credential rotation or provider side effects.
use crate::database::{self, documents, snapshot};
use anyhow::{ensure, Context, Result};
use std::{collections::BTreeSet, path::Path};
use typedb_driver::{TransactionType, TypeDBDriver};
pub(crate) fn definitions(schema: &str) -> Result<BTreeSet<String>> {
    use typeql::{
        query::{schema::SchemaQuery, QueryStructure},
        schema::definable::Definable,
    };
    let mut out = BTreeSet::new();
    for query in typeql::parse_queries(schema)? {
        let QueryStructure::Schema(SchemaQuery::Define(define)) = query.structure else {
            anyhow::bail!("Unexpected schema query")
        };
        for def in define.definables {
            match def {
                Definable::TypeDeclaration(t) => {
                    out.insert(format!("type:{}", t.label));
                }
                Definable::Function(f) => {
                    out.insert(format!("function:{}", f.signature.ident));
                }
                _ => anyhow::bail!("Review unsupported custom schema before migration"),
            }
        }
    }
    Ok(out)
}
pub async fn import(
    driver: &TypeDBDriver,
    source: &str,
    target: &str,
    directory: &Path,
) -> Result<usize> {
    ensure!(
        matches!(source, "business_native_dev" | "business_dev")
            || source.starts_with("business_backend_test_"),
        "Development source only"
    );
    database::target(target)?;
    ensure!(
        target == "business_native_dev_v2" || target.starts_with("business_backend_test_"),
        "Use a new development shadow database"
    );
    ensure!(
        source != target && !driver.databases().contains(target).await?,
        "Target exists; nothing overwritten"
    );
    let old = std::fs::read_to_string(directory.join(format!("{source}.tql")))?;
    let schema = database::import_schema();
    let unknown: Vec<_> = definitions(&old)?
        .difference(&definitions(&schema)?)
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
        "Custom source schema requires preservation review: {unknown:?}"
    );
    let expected: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(directory.join(format!("{source}.records.json")))?,
    )?;
    driver
        .databases()
        .import_from_file(target, schema, directory.join(format!("{source}.typedb")))
        .await?;
    // A failed rehearsal is retained for inspection. The original stays untouched.
    let reference = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    driver
        .databases()
        .import_from_file(&reference, old, directory.join(format!("{source}.typedb")))
        .await?;
    let original = snapshot::records(driver, &reference).await?;
    let mut expected_values: Vec<_> = expected
        .values()
        .map(serde_json::to_string)
        .collect::<std::result::Result<_, _>>()?;
    expected_values.sort();
    let mut restored_values: Vec<_> = original
        .values()
        .map(serde_json::to_string)
        .collect::<std::result::Result<_, _>>()?;
    restored_values.sort();
    ensure!(
        expected_values == restored_values,
        "Backup records do not match the restored source"
    );
    let before = snapshot::graph(driver, &reference, &[]).await?;
    ensure!(
        snapshot::graph(driver, target, &[]).await? == before,
        "Shadow import changed logical objects or relation role bindings; source is unchanged"
    );
    let preserved = snapshot::graph(driver, &reference, &["invitation_valid_until"]).await?;
    let tx = driver.transaction(target, TransactionType::Write).await?;
    let invitations=documents(&tx,"match $l isa account_invitation,has invitation_expires $expiry;not {$l has invitation_valid_until $until;};fetch {'id':$l.invitation_id,'expires':$expiry};").await?;
    for invitation in &invitations {
        let expiry = invitation["expires"]
            .as_i64()
            .context("Invalid invitation expiry")?;
        let until = chrono::DateTime::from_timestamp(expiry, 0)
            .context("Invitation expiry out of range")?
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6f")
            .to_string();
        tx.query(format!("match $l isa account_invitation,has invitation_id {},has invitation_expires {expiry};not {{$l has invitation_valid_until $until;}};insert $l has invitation_valid_until {until};",invitation["id"])).await?;
    }
    tx.commit().await?;
    ensure!(
        snapshot::graph(driver, target, &["invitation_valid_until"]).await? == preserved,
        "Original attributes or relation roles changed"
    );
    driver.databases().get(&reference).await?.delete().await?;
    database::verify_runtime(driver, target).await?;
    Ok(expected.len())
}
