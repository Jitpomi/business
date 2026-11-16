//! Explicit schema installation/import. Never resets a database or changes invoice records.
use anyhow::{ensure, Result};
use std::sync::Arc;
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let args: Vec<_> = std::env::args().collect();
    let command = args.get(1).map(String::as_str).unwrap_or("");
    ensure!(
        ["install", "import"].contains(&command),
        "Use install/import and an explicit database"
    );
    let target = args
        .get(2)
        .ok_or_else(|| anyhow::anyhow!("Explicit database required"))?;
    server::database::target(target)?;
    ensure!(
        std::env::var("AUTOMATION_WRITERS_STOPPED").as_deref() == Ok("true"),
        "Stop old workers and all database writers first"
    );
    let directory = std::path::PathBuf::from(std::env::var("AUTOMATION_BACKUP_DIR")?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            directory.metadata()?.permissions().mode() & 0o077 == 0,
            "Use a private backup directory (mode 700)"
        );
    }
    for extension in ["tql", "typedb", "records.json", "inventory.json"] {
        ensure!(
            directory
                .join(format!("{target}.{extension}"))
                .metadata()?
                .len()
                > 0,
            "A complete database backup is required"
        );
    }
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    server::database::verify(&driver, target).await?;
    let state = Arc::new(server::typedb::TypeDBState {
        driver,
        database: target.into(),
    });
    let before: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(directory.join(format!("{target}.records.json")))?,
    )?;
    let current = server::database::snapshot::records(&state.driver, target).await?;
    preserve(&before, &current, false)?;
    if command == "install" {
        server::automation::store::install(&state).await?;
        preserve(
            &before,
            &server::database::snapshot::records(&state.driver, target).await?,
            false,
        )?;
        println!("Installed additive automation schema; existing records verified");
        return Ok(());
    }
    ensure!(
        server::automation::store::installed(&state).await?,
        "Install the automation schema first"
    );
    let tenant = std::env::var("AUTOMATION_IMPORT_TENANT")?;
    let owner = std::env::var("AUTOMATION_IMPORT_OWNER")?;
    let mut catalog = server::automation::connectors::Catalog::default();
    for kind in ["stripe", "mercury", "resend"] {
        catalog.register(Arc::new(server::automation::builtin::Builtin(kind)))?;
    }
    let vault = server::automation::vault::Vault::from_env()?;
    let registry = Arc::new(
        server::finance::TenantRegistry::default().with_source(Arc::new(
            server::automation::managed::Managed {
                state: state.clone(),
                catalog: Arc::new(catalog.clone()),
                vault: vault.clone(),
            },
        )),
    );
    catalog.register(Arc::new(server::automation::billing::Billing {
        state: state.clone(),
        registry,
    }))?;
    let engine = server::automation::engine::Engine {
        store: server::automation::store::Store::new(state.clone(), &tenant)?,
        catalog: Arc::new(catalog),
        vault,
    };
    server::automation::migration::import(&engine, &owner).await?;
    preserve(
        &before,
        &server::database::snapshot::records(&state.driver, target).await?,
        true,
    )?;
    println!("Imported tenant connections and billing workflow; provider aliases preserved; no payments or emails sent");
    Ok(())
}

fn preserve(
    before: &std::collections::BTreeMap<String, serde_json::Value>,
    after: &std::collections::BTreeMap<String, serde_json::Value>,
    allow_automation: bool,
) -> Result<()> {
    for (iid, old) in before {
        if allow_automation && old.get("auto_key").is_some() {
            continue;
        }
        let new = after
            .get(iid)
            .ok_or_else(|| anyhow::anyhow!("Existing record missing"))?;
        ensure!(
            old.as_object().map(|o| o.len()) == new.as_object().map(|o| o.len()),
            "Existing record shape changed"
        );
        for (key, value) in old
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Invalid backup"))?
        {
            if allow_automation && key == "revision" && old.get("tenant_id").is_some() {
                continue;
            }
            if let (Some(a), Some(b)) = (value.as_array(), new[key].as_array()) {
                ensure!(
                    a.len() == b.len() && a.iter().all(|v| b.contains(v)),
                    "Existing attributes changed"
                );
            } else {
                ensure!(value == &new[key], "Existing attribute changed");
            }
        }
    }
    for (iid, value) in after {
        if !before.contains_key(iid) {
            ensure!(
                allow_automation && value.get("auto_key").is_some(),
                "Unexpected record appeared; keep writers stopped and investigate"
            );
        }
    }
    Ok(())
}
