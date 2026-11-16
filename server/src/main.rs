use anyhow::Result;
#[tokio::main]
async fn main() -> Result<()> {
    let (app, http) = server::build().await?;
    let host: String = app.get("http.host").unwrap();
    let port: String = app.get("http.port").unwrap();
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    let mut router = server::app::router(http);
    let automation: Option<std::sync::Arc<server::automation::engine::Engine>> =
        app.get("automation.engine");
    if let Some(template) = automation.clone() {
        anyhow::ensure!(
            server::automation::store::installed(&template.store.state).await?,
            "Install the additive automation schema before enabling automation"
        );
        router = router.merge(server::automation::events::router((*template).clone()));
    }
    if std::env::var("BILLING_ENABLED").as_deref() == Ok("true") {
        anyhow::ensure!(automation.is_some(),"Billing requires the automation engine; install/import connections and billing workflows before enabling it");
        let state = app
            .get("typedb")
            .ok_or_else(|| anyhow::anyhow!("Missing TypeDB state"))?;
        let registry = app
            .get("tenant.plugins")
            .ok_or_else(|| anyhow::anyhow!("Missing tenant registry"))?;
        router = router.merge(server::billing::http::managed_router(
            state,
            registry,
            std::env::var("ACCOUNT_TENANT_ID").ok(),
        )?);
    }
    let worker =
        automation.map(|template| server::automation::runtime::spawn_all((*template).clone()));
    let served = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("Install SIGTERM handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = terminate.recv() => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        })
        .await;
    if let Some(worker) = worker {
        worker.shutdown().await?;
    }
    served?;
    Ok(())
}
