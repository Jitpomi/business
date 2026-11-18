//! Explicit operational tooling. No payment execution or startup migrations.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("seed-canary") => println!("{}", server::operations::seed_canary().await?),
        Some("refresh-control-policy") => {
            anyhow::ensure!(
                args.len() == 4 && std::env::var("TYPEDB_TLS").as_deref() == Ok("true"),
                "Explicit development target, verified checkpoint and TLS required"
            );
            let driver = dog_typedb::TypeDBDriverFactory::connect(
                &std::env::var("TYPEDB_ADDR")?,
                &std::env::var("TYPEDB_USERNAME")?,
                &std::env::var("TYPEDB_PASSWORD")?,
                true,
            )
            .await?;
            server::operations::refresh_control_policy(
                &driver,
                &args[2],
                std::path::Path::new(&args[3]),
            )
            .await?;
            println!("Native control policy updated; complete checkpoint graph unchanged");
        }
        _ => anyhow::bail!(
            "Use seed-canary or refresh-control-policy DEVELOPMENT_DATABASE GRAPH_CHECKPOINT.json"
        ),
    }
    Ok(())
}
