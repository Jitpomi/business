use anyhow::Result;
#[tokio::main]
async fn main() -> Result<()> {
    let (app, http) = server::build().await?;
    let host: String = app.get("http.host").unwrap();
    let port: String = app.get("http.port").unwrap();
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    let mut router = server::app::router(http);
    let mut billing_workers = Vec::new();
    if std::env::var("BILLING_ENABLED").as_deref() == Ok("true") {
        let tenants: Vec<String> =
            serde_json::from_str(&std::env::var("BILLING_WORKER_TENANTS_JSON")?)?;
        anyhow::ensure!(
            !tenants.is_empty(),
            "Configure explicit billing worker tenants"
        );
        let primary = std::env::var("ACCOUNT_TENANT_ID").ok();
        let mut seen = std::collections::HashSet::new();
        let registry: std::sync::Arc<server::finance::TenantRegistry> =
            app.get("tenant.plugins")
                .ok_or_else(|| anyhow::anyhow!("Tenant registry missing"))?;
        for tenant in tenants {
            let tenant = server::billing::billing_schema::id(&tenant)?;
            anyhow::ensure!(seen.insert(tenant.clone()), "Duplicate worker tenant");
            let state = app
                .get("typedb")
                .ok_or_else(|| anyhow::anyhow!("Missing TypeDB state"))?;
            let engine =
                server::billing::engine::Engine::new(state, &tenant, registry.get(&tenant)?)?;
            let (routes, worker) = server::billing::http::runtime(engine)?;
            router = router.nest(&format!("/tenants/{tenant}"), routes.clone());
            if primary.as_deref() == Some(tenant.as_str()) {
                router = router.merge(routes);
            }
            billing_workers.push(worker);
        }
        anyhow::ensure!(
            primary.as_ref().is_none_or(|p| seen.contains(p)),
            "Account tenant must be in the worker list"
        );
    }
    axum::serve(listener, router)
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
        .await?;
    for worker in billing_workers {
        worker.abort();
        let _ = worker.await;
    }
    Ok(())
}
