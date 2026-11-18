//! Explicit operational canary creation; no tenant data, payment or schema mutation.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    anyhow::ensure!(
        std::env::args().nth(1).as_deref() == Some("seed-canary"),
        "Use operations seed-canary with a private environment file"
    );
    println!("{}", server::operations::seed_canary().await?);
    Ok(())
}
