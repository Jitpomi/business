//! Shared read-only database preservation evidence.
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use typedb_driver::{TransactionType, TypeDBDriver};
pub async fn inventory(driver: &TypeDBDriver, name: &str) -> Result<Value> {
    let schema = driver.databases().get(name).await?.schema().await?;
    let tx = driver.transaction(name, TransactionType::Read).await?;
    let mut counts = BTreeMap::new();
    for line in schema.lines() {
        if let Some(label) = line
            .trim()
            .strip_prefix("entity ")
            .or_else(|| line.trim().strip_prefix("relation "))
        {
            let label = label.split([',', ' ', ';']).next().unwrap();
            let rows = crate::database::documents(
                &tx,
                &format!("match $x isa {label};reduce $n=count;fetch {{'count':$n}};"),
            )
            .await?;
            counts.insert(label.to_owned(), rows[0]["count"].clone());
        }
    }
    tx.close().await?;
    Ok(
        json!({"database":name,"canonical":schema.contains("scope_bill_customer"),"plugin_schema":schema.contains("attribute bill_bindings"),"counts":counts}),
    )
}
pub async fn records(driver: &TypeDBDriver, name: &str) -> Result<BTreeMap<String, Value>> {
    let schema = driver.databases().get(name).await?.schema().await?;
    let tx = driver.transaction(name, TransactionType::Read).await?;
    let mut records = BTreeMap::new();
    for line in schema.lines() {
        if let Some(label) = line
            .trim()
            .strip_prefix("entity ")
            .or_else(|| line.trim().strip_prefix("relation "))
        {
            let label = label.split([',', ' ', ';']).next().unwrap();
            for row in crate::database::documents(
                &tx,
                &format!("match $x isa {label};fetch {{'iid':iid($x),'data':{{$x.*}}}};"),
            )
            .await?
            {
                records.insert(
                    row["iid"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing internal ID"))?
                        .to_owned(),
                    row["data"].clone(),
                );
            }
        }
    }
    tx.close().await?;
    Ok(records)
}
