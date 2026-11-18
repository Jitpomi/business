use anyhow::Result;
#[tokio::main]
async fn main() -> Result<()> {
    // Install before startup so framework queue/recovery diagnostics reach operators.
    // The default excludes driver debug traces and request/credential payloads.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init()
        .map_err(|_| anyhow::anyhow!("Could not initialize application logging"))?;
    let (app, http) = server::build().await?;
    let host: String = app.get("http.host").unwrap();
    let port: String = app.get("http.port").unwrap();
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    let router = server::app::runtime_router(&app, http).await?;
    let automation: Option<std::sync::Arc<server::automation::engine::Engine>> =
        app.get("automation.engine");
    let worker = automation.map(|template| server::background::spawn_all((*template).clone()));
    tracing::info!("Business HTTP services and configured workers started");
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
    tracing::info!("Business requests and workers drained");
    Ok(())
}
