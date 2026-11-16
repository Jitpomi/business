//! Narrow server-to-server ingress; no tenant header controls merchant identity.
use super::engine::Engine;
use anyhow::Result;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
#[derive(Clone)]
struct Ingress {
    engine: Engine,
    slots: Arc<tokio::sync::Semaphore>,
    account_slots: Arc<tokio::sync::Semaphore>,
}
fn configured(engine: Engine) -> Result<Ingress> {
    Ok(Ingress {
        engine,
        slots: Arc::new(tokio::sync::Semaphore::new(1)),
        account_slots: Arc::new(tokio::sync::Semaphore::new(4)),
    })
}
fn routes(state: Ingress) -> Router {
    Router::new()
        .route("/webhooks/{plugin}", post(webhook))
        .route("/onboarding", post(accept_invitation))
        .route("/recovery", post(recover_account))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
}
pub fn router(engine: Engine) -> Result<Router> {
    Ok(routes(configured(engine)?))
}
async fn webhook(
    State(state): State<Ingress>,
    Path(plugin_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    let plugin = state
        .engine
        .runtime
        .financial(&plugin_id)
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let headers = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
        .collect();
    let _slot = state
        .slots
        .try_acquire()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(45), async {
        let observation = plugin.webhook(&headers, &body).await.map_err(|e| match e {
            crate::finance::WebhookError::Unauthorized => StatusCode::UNAUTHORIZED,
            crate::finance::WebhookError::Invalid => StatusCode::BAD_REQUEST,
            crate::finance::WebhookError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })?;
        if let Some(observation) = observation {
            state
                .engine
                .apply_observation(&observation.invoice_id, &plugin_id, &observation)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        }
        Ok::<_, StatusCode>(Json(json!({"received":true})))
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    result
}
async fn account_action(
    state: Ingress,
    headers: HeaderMap,
    data: Value,
    kind: &str,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let secret = std::env::var("BUSINESS_GATEWAY_SECRET").unwrap_or_default();
    if !crate::admission::gateway_matches(&headers, &secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"message":"Please use the account website."})),
        ));
    }
    // Bound expensive password work independently of the billing worker.
    let _slot = state.account_slots.try_acquire().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"message":"Please wait a moment and try again."})),
        )
    })?;
    // Gateway has per-IP limits; the DB additionally limits reset mail per account.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(40),
        crate::auth::AccountLifecycle::new(
            state.engine.state.clone(),
            &state.engine.merchant,
            state.engine.runtime.clone(),
        )
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message":"Account service unavailable"})),
            )
        })?
        .public_account_action(kind, &data),
    )
    .await;
    match result {
        Ok(Ok(value)) => Ok(Json(value)),
        _ => Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"message":"We couldn’t use that link. Check your details or request a new link."}),
            ),
        )),
    }
}
async fn accept_invitation(
    State(state): State<Ingress>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if data["action"] != "accept" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"message":"Action unavailable"})),
        ));
    }
    account_action(state, headers, data, "invite").await
}
async fn recover_account(
    State(state): State<Ingress>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if data["action"] != "request" && data["action"] != "reset" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"message":"Action unavailable"})),
        ));
    }
    account_action(state, headers, data, "reset").await
}
/// Dynamic ingress uses the same tenant-owned connections as automation and the financial proxy.
#[derive(Clone)]
struct ManagedIngress {
    state: Arc<crate::typedb::TypeDBState>,
    registry: Arc<crate::finance::TenantRegistry>,
    slots: Arc<tokio::sync::Semaphore>,
    accounts: Arc<tokio::sync::Semaphore>,
}
impl ManagedIngress {
    async fn tenant(&self, id: &str) -> Result<Ingress, StatusCode> {
        let id = super::billing_schema::id(id).map_err(|_| StatusCode::NOT_FOUND)?;
        let runtime = self
            .registry
            .resolve(&id)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let engine =
            Engine::new(self.state.clone(), &id, runtime).map_err(|_| StatusCode::NOT_FOUND)?;
        let mut ingress = configured(engine).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        ingress.slots = self.slots.clone();
        ingress.account_slots = self.accounts.clone();
        Ok(ingress)
    }
}
pub fn managed_router(
    state: Arc<crate::typedb::TypeDBState>,
    registry: Arc<crate::finance::TenantRegistry>,
    primary: Option<String>,
) -> Result<Router> {
    let state = ManagedIngress {
        state,
        registry,
        slots: Arc::new(tokio::sync::Semaphore::new(4)),
        accounts: Arc::new(tokio::sync::Semaphore::new(4)),
    };
    let mut routes = Router::new()
        .route("/tenants/{tenant}/webhooks/{plugin}", post(managed_webhook))
        .route("/tenants/{tenant}/onboarding", post(managed_onboarding))
        .route("/tenants/{tenant}/recovery", post(managed_recovery))
        .layer(DefaultBodyLimit::max(65536))
        .with_state(state.clone());
    if let Some(primary) = primary {
        super::billing_schema::id(&primary)?;
        let alias = Router::new()
            .route("/webhooks/{plugin}", post(primary_webhook))
            .route("/onboarding", post(primary_onboarding))
            .route("/recovery", post(primary_recovery))
            .layer(DefaultBodyLimit::max(65536))
            .with_state((state, primary));
        routes = routes.merge(alias);
    }
    Ok(routes.route_layer(axum::middleware::from_fn(crate::admission::external_guard)))
}
async fn managed_webhook(
    State(state): State<ManagedIngress>,
    Path((tenant, plugin)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    webhook(
        State(state.tenant(&tenant).await?),
        Path(plugin),
        headers,
        body,
    )
    .await
}
async fn managed_onboarding(
    State(state): State<ManagedIngress>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let state = state
        .tenant(&tenant)
        .await
        .map_err(|s| (s, Json(json!({"message":"Account unavailable"}))))?;
    accept_invitation(State(state), headers, Json(data)).await
}
async fn managed_recovery(
    State(state): State<ManagedIngress>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let state = state
        .tenant(&tenant)
        .await
        .map_err(|s| (s, Json(json!({"message":"Account unavailable"}))))?;
    recover_account(State(state), headers, Json(data)).await
}

async fn primary_webhook(
    State((s, t)): State<(ManagedIngress, String)>,
    Path(plugin): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    managed_webhook(State(s), Path((t, plugin)), headers, body).await
}
async fn primary_onboarding(
    State((s, t)): State<(ManagedIngress, String)>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    managed_onboarding(State(s), Path(t), headers, Json(data)).await
}
async fn primary_recovery(
    State((s, t)): State<(ManagedIngress, String)>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    managed_recovery(State(s), Path(t), headers, Json(data)).await
}
