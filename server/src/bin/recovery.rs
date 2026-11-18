//! Explicit recovery commands. No schema reset, existing-target restore, worker or provider execution.
use anyhow::{ensure, Result};
use std::path::Path;
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let a = |n: usize| {
        args.get(n)
            .map(String::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing explicit recovery argument"))
    };
    match a(1)? {
        "encrypt" => {
            server::recovery::encrypt(Path::new(a(2)?), Path::new(a(3)?), Path::new(a(4)?))
        }
        "decrypt" => {
            server::recovery::decrypt(Path::new(a(2)?), Path::new(a(3)?), Path::new(a(4)?))
        }
        command => {
            dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
            ensure!(
                std::env::var("TYPEDB_TLS")? == "true",
                "Recovery tooling requires verified TLS"
            );
            let driver = dog_typedb::TypeDBDriverFactory::connect(
                &std::env::var("TYPEDB_ADDR")?,
                &std::env::var("TYPEDB_USERNAME")?,
                &std::env::var("TYPEDB_PASSWORD")?,
                true,
            )
            .await?;
            match command {
    "rehearse"=>{let report=server::recovery::rehearse(&driver,Path::new(a(2)?),a(3)?).await?;println!("{report}");Ok(())},
    "restore"=>{server::recovery::restore(&driver,Path::new(a(2)?),a(3)?,a(4)?).await?;println!("New recovery shadow matches the complete checkpoint graph and native schema");Ok(())},
    "verify-vault"=>{let state=server::typedb::TypeDBState{driver:driver.into(),database:a(2)?.into()};let report=server::recovery::verify_vault(&state,Path::new(a(3)?),&std::env::var("AUTOMATION_MASTER_KEY_B64")?,std::env::var("OPERATIONS_VAULT_CANARY_REF").ok().as_deref()).await?;println!("{report}");Ok(())},
    _=>anyhow::bail!("Use encrypt INPUT OUTPUT KEY, decrypt INPUT OUTPUT KEY, restore CHECKPOINT SOURCE NEW_SHADOW, or verify-vault SHADOW SQL")
   }
        }
    }
}
