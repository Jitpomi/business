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
use subtle::ConstantTimeEq;
#[derive(Clone)]
struct Ingress {
    engine: Engine,
    tick_secret: String,
    slots: Arc<tokio::sync::Semaphore>,
    account_slots: Arc<tokio::sync::Semaphore>,
}
fn configured(engine: Engine) -> Result<Ingress> {
    let tick_secret = engine.runtime.scheduler_secret.clone().unwrap_or_default();
    Ok(Ingress {
        engine,
        tick_secret,
        slots: Arc::new(tokio::sync::Semaphore::new(1)),
        account_slots: Arc::new(tokio::sync::Semaphore::new(4)),
    })
}
fn routes(state: Ingress) -> Router {
    Router::new()
        .route("/internal/billing/tick", post(tick))
        .route("/webhooks/{plugin}", post(webhook))
        .route("/onboarding", post(accept_invitation))
        .route("/recovery", post(recover_account))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
}
pub fn router(engine: Engine) -> Result<Router> {
    Ok(routes(configured(engine)?))
}
/// One worker and ingress share admission; database claims also protect replicas.
/// The caller owns and aborts the worker on shutdown. No task is started by router().
pub fn runtime(engine: Engine) -> Result<(Router, tokio::task::JoinHandle<()>)> {
    let state = configured(engine)?;
    let router = routes(state.clone());
    let worker = tokio::spawn(async move {
        let mut clock = tokio::time::interval(std::time::Duration::from_secs(60));
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            clock.tick().await;
            let Ok(_slot) = state.slots.try_acquire() else {
                continue;
            };
            match tokio::time::timeout(std::time::Duration::from_secs(300), state.engine.tick())
                .await
            {
                Ok(Ok(report)) => {
                    tracing::info!(merchant=%state.engine.merchant, report=%report, "Billing tick complete");
                }
                Ok(Err(_)) => {
                    tracing::error!(merchant=%state.engine.merchant, "Billing tick failed; inspect durable claims before retrying uncertain effects");
                }
                Err(_) => {
                    tracing::error!(merchant=%state.engine.merchant, "Billing tick deadline exceeded; durable claims preserved");
                }
            }
        }
    });
    Ok((router, worker))
}

async fn tick(
    State(state): State<Ingress>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let received = headers
        .get("authorization")
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if state.tick_secret.len() < 32
        || !bool::from(state.tick_secret.as_bytes().ct_eq(received.as_bytes()))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let _slot = state
        .slots
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    // Cancellation preserves durable claims. No dropped worker can replay a charge.
    let result = tokio::time::timeout(std::time::Duration::from_secs(50), state.engine.tick())
        .await
        .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let status = if result["errors"].as_u64().unwrap_or(1) == 0 {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    Ok((status, Json(result)))
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
        state.engine.public_account_action(kind, &data),
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
