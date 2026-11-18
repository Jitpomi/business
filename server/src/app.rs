use crate::BusinessParams;
use anyhow::Result;
use dog_core::DogApp;
use dog_transport::{http::DogHttpService, IntoDogService};
use serde_json::Value;
use std::sync::Arc;
pub type Backend = (
    DogApp<Value, BusinessParams>,
    DogHttpService<Value, BusinessParams>,
);
/// Startup only connects and verifies; provisioning is an explicit operator action.
pub async fn build() -> Result<Backend> {
    crate::tls::initialize();
    if let Err(error) = match std::env::var("BUSINESS_ENV_FILE") {
        Ok(path) => dotenvy::from_path(path),
        Err(_) => dotenvy::dotenv().map(|_| ()),
    } {
        if !error.not_found() {
            return Err(error.into());
        }
    }
    let mut builder = dog_core::DogAppBuilder::new();
    crate::config::config(&mut builder)?;
    crate::typedb::TypeDBState::initialize(&mut builder).await?;
    compose(builder, configured_registry()?, None)
}
/// The same composition used by the executable, tools and integration tests.
pub fn build_with_state(
    state: Arc<crate::typedb::TypeDBState>,
    jwt_secret: String,
) -> Result<Backend> {
    build_with_plugins(state, jwt_secret, configured_registry()?)
}
/// Hosts can register any trusted plugin implementation without editing business services.
pub fn build_with_plugins(
    state: Arc<crate::typedb::TypeDBState>,
    jwt_secret: String,
    registry: Arc<crate::finance::TenantRegistry>,
) -> Result<Backend> {
    anyhow::ensure!(
        jwt_secret.trim().len() >= 32,
        "JWT secret must contain at least 32 nonblank bytes"
    );
    let mut builder = dog_core::DogAppBuilder::new();
    builder.set("auth.jwt.secret", jwt_secret);
    builder.set("typedb", state);
    compose(builder, registry, None)
}
fn compose(
    mut builder: dog_core::DogAppBuilder<Value, BusinessParams>,
    mut registry: Arc<crate::finance::TenantRegistry>,
    connectors: Option<crate::automation::connectors::Catalog>,
) -> Result<Backend> {
    crate::tls::initialize();
    let state: Arc<crate::typedb::TypeDBState> = builder
        .get("typedb")
        .ok_or_else(|| anyhow::anyhow!("TypeDB state missing"))?;
    if std::env::var("AUTOMATION_ENABLED").as_deref() == Ok("true") {
        let mut catalog = connectors.unwrap_or_default();
        catalog.register(Arc::new(crate::automation::connectors::Http))?;
        crate::plugins::connectors::register(&mut catalog)?;
        let vault = crate::automation::vault::Vault::from_env()?;
        registry = Arc::new((*registry).clone().with_source(Arc::new(
            crate::automation::managed::Managed {
                state: state.clone(),
                catalog: Arc::new(catalog.clone()),
                vault: vault.clone(),
            },
        )));
        if std::env::var("BILLING_ENABLED").as_deref() == Ok("true") {
            catalog.register(Arc::new(crate::automation::billing::Billing {
                state: state.clone(),
                registry: registry.clone(),
            }))?;
        }
        builder.set(
            "automation.engine",
            Arc::new(crate::automation::engine::Engine {
                store: crate::automation::store::Store::new(
                    state.clone(),
                    "00000000-0000-0000-0000-000000000000",
                )?,
                catalog: Arc::new(catalog),
                vault,
            }),
        );
    }
    builder.set("tenant.plugins", registry.clone());
    let auth = crate::auth::strategies(&mut builder, state.clone())?;
    crate::services::configure(&mut builder, auth.clone(), state, registry)?;
    let authentication_requirements = crate::auth::requirements::configure(&mut builder)?;
    crate::hooks::register_global_hooks(&mut builder)?;
    crate::channels::configure(&mut builder)?;
    let app = builder.build();
    for service in authentication_requirements {
        app.service(&service).map_err(|_| {
            anyhow::anyhow!("Authentication requirement names an unregistered service")
        })?;
    }
    auth.setup(app.clone());
    let http = app.clone().into_service(
        dog_transport::HttpOptions::default()
            .tenant_header("x-tenant-id")
            .body_limit(65536),
    );
    Ok((app, http))
}
dog_transport::declare_adapter!(axum, endpoint, crate::BusinessParams);
pub fn router(http: DogHttpService<Value, BusinessParams>) -> axum::Router {
    axum::Router::new()
        .route_service("/authentication", endpoint(http.clone()))
        .route_service("/data", endpoint(http.clone()))
        .route(
            "/financial",
            axum::routing::get_service(endpoint(http.clone())).post_service(endpoint(http.clone())),
        )
        .route_service("/automation", endpoint(http.clone()))
        .merge(crate::plugins::routes(http))
        .route_layer(axum::middleware::from_fn(crate::hooks::guard))
        .route(
            "/queries/data/{name}",
            axum::routing::get(
                |axum::extract::Path(name): axum::extract::Path<String>| async move {
                    let name = name.strip_suffix(".tql").unwrap_or(&name);
                    match crate::services::data::query_assets::ASSETS
                        .iter()
                        .find(|(asset, _)| *asset == name)
                    {
                        Some((_, source)) => (
                            axum::http::StatusCode::OK,
                            [(
                                axum::http::header::CONTENT_TYPE,
                                "text/plain; charset=utf-8",
                            )],
                            *source,
                        ),
                        None => (
                            axum::http::StatusCode::NOT_FOUND,
                            [(
                                axum::http::header::CONTENT_TYPE,
                                "text/plain; charset=utf-8",
                            )],
                            "",
                        ),
                    }
                },
            ),
        )
        .route(
            "/data-client.js",
            axum::routing::get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../public/data-client.js"),
                )
            }),
        )
        .route("/health", axum::routing::get(|| async { "ok" }))
}

fn configured_registry() -> Result<Arc<crate::finance::TenantRegistry>> {
    if std::env::var("AUTOMATION_ENABLED").as_deref() == Ok("true") {
        Ok(Arc::new(crate::finance::TenantRegistry::default()))
    } else {
        crate::plugins::configuration::registry()
    }
}

/// Install trusted connector packages without editing the automation engine or services.
pub fn build_with_connectors(
    state: Arc<crate::typedb::TypeDBState>,
    jwt_secret: String,
    registry: Arc<crate::finance::TenantRegistry>,
    catalog: crate::automation::connectors::Catalog,
) -> Result<Backend> {
    anyhow::ensure!(
        std::env::var("AUTOMATION_ENABLED").as_deref() == Ok("true"),
        "Enable automation before installing connectors"
    );
    anyhow::ensure!(
        jwt_secret.trim().len() >= 32,
        "JWT secret must contain at least 32 nonblank bytes"
    );
    let mut builder = dog_core::DogAppBuilder::new();
    builder.set("auth.jwt.secret", jwt_secret);
    builder.set("typedb", state);
    compose(builder, registry, Some(catalog))
}

/// Hosts can supply provider plugins without changing data or financial services.
pub fn build_with_authentication(
    state: Arc<crate::typedb::TypeDBState>,
    jwt_secret: String,
    finance: Arc<crate::finance::TenantRegistry>,
    authentication: Arc<crate::auth::Registry>,
) -> Result<Backend> {
    anyhow::ensure!(
        jwt_secret.trim().len() >= 32,
        "JWT secret must contain at least 32 bytes"
    );
    let mut builder = dog_core::DogAppBuilder::new();
    builder.set("typedb", state);
    builder.set("auth.jwt.secret", jwt_secret);
    builder.set("authentication.plugins", authentication);
    compose(builder, finance, None)
}

/// Compose every HTTP ingress using the same application state as registered services.
/// Worker lifetime remains owned by the process entry point.
pub async fn runtime_router(
    app: &DogApp<Value, BusinessParams>,
    http: DogHttpService<Value, BusinessParams>,
) -> Result<axum::Router> {
    let mut router = router(http);
    let automation: Option<std::sync::Arc<crate::automation::engine::Engine>> =
        app.get("automation.engine");
    if let Some(template) = automation.clone() {
        anyhow::ensure!(
            crate::automation::store::installed(&template.store.state).await?,
            "Install the additive automation schema before enabling automation"
        );
        router = router.merge(crate::automation::events::router((*template).clone()));
    }
    if std::env::var("BILLING_ENABLED").as_deref() == Ok("true") {
        anyhow::ensure!(automation.is_some(),"Billing requires the automation engine; install/import connections and billing workflows before enabling it");
        let state = app
            .get("typedb")
            .ok_or_else(|| anyhow::anyhow!("Missing TypeDB state"))?;
        let registry = app
            .get("tenant.plugins")
            .ok_or_else(|| anyhow::anyhow!("Missing tenant registry"))?;
        router = router.merge(crate::billing::http::managed_router(
            state,
            registry,
            std::env::var("ACCOUNT_TENANT_ID").ok(),
        )?);
    }
    if std::env::var("OPERATIONS_ENABLED").as_deref() == Ok("true") {
        let state = app
            .get("typedb")
            .ok_or_else(|| anyhow::anyhow!("Missing TypeDB state"))?;
        router = router.merge(crate::operations::router(state)?);
    }
    if crate::operations::recovery_mode()? {
        router = router.layer(axum::middleware::from_fn(crate::operations::recovery_guard));
    }
    Ok(router)
}
