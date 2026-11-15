//! Explicit, resumable schema/data migration. Never called by server startup.
use crate::database::{documents, target};
use anyhow::{ensure, Result};
use std::collections::BTreeMap;
use typedb_driver::{TransactionType, TypeDBDriver};
fn q(v: &str) -> String {
    serde_json::to_string(v).unwrap()
}
pub async fn migrate(driver: &TypeDBDriver, name: &str) -> Result<usize> {
    target(name)?;
    let schema = driver.databases().get(name).await?.schema().await?;
    ensure!(
        schema.contains("bill_customer") && schema.contains("scope_bill_customer"),
        "Canonical billing model required"
    );
    let compact: String = schema.chars().filter(|c| !c.is_whitespace()).collect();
    let tx = driver.transaction(name, TransactionType::Schema).await?;
    if !schema.contains("attribute bill_bindings") {
        tx.query("define attribute bill_bindings @meta('dog.security.hidden','true'),value string;attribute bill_method_plugin @meta('dog.security.hidden','true'),value string;entity bill_customer owns bill_bindings @card(0..1),owns bill_method_plugin @card(0..1);").await?;
    }
    if compact.contains("ownsbill_stripe_id@card(1..1)") {
        tx.query("redefine bill_customer owns bill_stripe_id @card(0..1);")
            .await?;
    }
    if compact.contains("ownsbill_mercury_id@card(1..1)") {
        tx.query("redefine bill_customer owns bill_mercury_id @card(0..1);")
            .await?;
    }
    if compact.contains("bill_provider,valuestring@values(")
        || compact.contains("bill_providervaluestring@values(")
    {
        tx.query("undefine @values from bill_provider value string;")
            .await?;
    }
    tx.commit().await?;
    let tx = driver.transaction(name, TransactionType::Write).await?;
    let rows=documents(&tx,"match $c isa bill_customer;fetch {'id':$c.bill_key,'stripe':[$c.bill_stripe_id],'mercury':[$c.bill_mercury_id],'bindings':[$c.bill_bindings],'plugin':[$c.bill_method_plugin],'method':$c.bill_payment_method,'removed':[$c.bill_removed_method]};").await?;
    let mut changed = 0;
    for row in rows {
        let existing = row["bindings"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid bindings projection"))?;
        let mut bindings: BTreeMap<String, String> = if let Some(v) = existing.first() {
            serde_json::from_str(
                v.as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid binding map"))?,
            )?
        } else {
            BTreeMap::new()
        };
        for id in ["stripe", "mercury"] {
            if let Some(value) = row[id]
                .as_array()
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                if let Some(previous) = bindings.get(id) {
                    ensure!(
                        previous == value,
                        "Existing plugin binding conflicts with legacy reference"
                    );
                } else {
                    bindings.insert(id.into(), value.into());
                }
            }
        }
        let method_plugin = row["plugin"]
            .as_array()
            .and_then(|v| v.first())
            .and_then(|v| v.as_str())
            .unwrap_or(if row["method"] == "" { "" } else { "stripe" });
        let id = row["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing customer key"))?;
        let serialized = serde_json::to_string(&bindings)?;
        if existing.first().and_then(|v| v.as_str()) != Some(&serialized)
            || row["plugin"].as_array().is_some_and(|v| v.is_empty())
        {
            tx.query(format!("match $c isa bill_customer,has bill_key {};update $c has bill_bindings {},has bill_method_plugin {};",q(id),q(&serialized),q(method_plugin))).await?;
            changed += 1;
        }
        for removed in row["removed"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid removal projection"))?
        {
            let value = removed
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid removed method"))?;
            if !value.contains(':') {
                tx.query(format!("match $c isa bill_customer,has bill_key {};insert $c has bill_removed_method {};",q(id),q(&format!("stripe:{value}")))).await?;
            }
        }
    }
    tx.commit().await?;
    let tx = driver.transaction(name, TransactionType::Schema).await?;
    let schema = driver.databases().get(name).await?.schema().await?;
    let compact: String = schema.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.contains("ownsbill_bindings@card(0..1)") {
        tx.query("redefine bill_customer owns bill_bindings @card(1);")
            .await?;
    }
    if compact.contains("ownsbill_method_plugin@card(0..1)") {
        tx.query("redefine bill_customer owns bill_method_plugin @card(1);")
            .await?;
    }
    tx.commit().await?;
    crate::database::verify(driver, name).await?;
    Ok(changed)
}
