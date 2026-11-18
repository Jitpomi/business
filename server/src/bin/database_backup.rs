//! Development-only backup and restore verification. Never overwrites a database.
use anyhow::{ensure, Result};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

use server::database::snapshot::{inventory, records};
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(String::as_str).unwrap_or("");
    ensure!(
        [
            "inspect",
            "export",
            "export-online",
            "restore-verify",
            "verify-preserved"
        ]
        .contains(&command),
        "Use inspect/export/restore-verify and an explicit development database"
    );
    let name = args
        .get(2)
        .ok_or_else(|| anyhow::anyhow!("Explicit database required"))?;
    server::database::target(name)?;
    ensure!(name != "business_prod", "Development databases only");
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    if command == "inspect" {
        println!("{}", inventory(&driver, name).await?);
        return Ok(());
    }
    let directory = Path::new(
        args.get(3)
            .ok_or_else(|| anyhow::anyhow!("Private backup directory required"))?,
    );
    ensure!(
        directory.is_dir(),
        "Create a private backup directory first"
    );
    let schema = directory.join(format!("{name}.tql"));
    let data = directory.join(format!("{name}.typedb"));
    let manifest = directory.join(format!("{name}.inventory.json"));
    if command == "verify-preserved" {
        let before: BTreeMap<String, Value> = serde_json::from_slice(&std::fs::read(
            directory.join(format!("{name}.records.json")),
        )?)?;
        let after = records(&driver, name).await?;
        ensure!(before.len() == after.len(), "Entity/relation count changed");
        for (iid, old) in &before {
            let new = after
                .get(iid)
                .ok_or_else(|| anyhow::anyhow!("Existing record missing"))?;
            for (key, value) in old
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("Invalid backup record"))?
            {
                if let (Some(old), Some(new)) = (value.as_array(), new[key].as_array()) {
                    ensure!(
                        old.iter().all(|v| new.contains(v)),
                        "Existing attribute value lost"
                    );
                } else {
                    ensure!(value == &new[key], "Existing attribute value changed");
                }
            }
        }
        let expected: Value = serde_json::from_slice(&std::fs::read(&manifest)?)?;
        ensure!(
            inventory(&driver, name).await?["counts"] == expected["counts"],
            "Model record counts changed"
        );
        server::database::verify(&driver, name).await?;
        println!("All {} existing entities/relations, their attributes and model record counts preserved; upgraded schema verified",before.len());
        return Ok(());
    }
    if command == "export-online" {
        let verifier = server::recovery::local_driver().await?;
        server::database::snapshot::online_checkpoint(&driver, &verifier, name, directory).await?;
        println!("Transactional online checkpoint exported and independently manifested");
    } else if command == "export" {
        server::database::snapshot::checkpoint(&driver, name, directory).await?;
        println!("Backup exported for {name}; restore verification still required");
    } else {
        let expected: Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
        let restored = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
        ensure!(
            !driver.databases().contains(&restored).await?,
            "Generated verification target already exists"
        );
        driver
            .databases()
            .import_from_file(&restored, std::fs::read_to_string(schema)?, data)
            .await?;
        let outcome = async {
            let actual = inventory(&driver, &restored).await?;
            ensure!(
                actual["counts"] == expected["counts"],
                "Restored record counts differ"
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;
        driver.databases().get(&restored).await?.delete().await?;
        outcome?;
        println!(
            "Backup restored and verified for {name}; disposable verification database removed"
        );
    }
    Ok(())
}
