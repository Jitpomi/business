//! Shared-secret event ingress; replay identity is scoped to tenant and trigger.
use super::{engine::Engine, model::identifier, store::Store};
use anyhow::{ensure, Result};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
impl Engine {
    pub async fn trigger(&self, id: &str, workflow: &str, enabled: bool) -> Result<Value> {
        identifier(id)?;
        identifier(workflow)?;
        ensure!(
            self.store.get("workflow", workflow).await?.is_some(),
            "Workflow unavailable"
        );
        if let Some(old) = self.store.get("trigger", id).await? {
            ensure!(
                old.value["workflow"] == workflow,
                "Create a new trigger ID when changing its workflow"
            );
            let value =
                json!({"id":id,"workflow":workflow,"enabled":enabled,"digest":old.value["digest"]});
            self.store
                .cas(
                    &old,
                    &value,
                    if enabled { "enabled" } else { "disabled" },
                    0,
                )
                .await?;
            return Ok(json!({"id":id,"workflow":workflow,"enabled":enabled}));
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let value = json!({"id":id,"workflow":workflow,"enabled":enabled,"digest":digest(&token)});
        self.store
            .insert(
                "trigger",
                id,
                &value,
                if enabled { "enabled" } else { "disabled" },
                0,
            )
            .await?;
        Ok(json!({"id":id,"workflow":workflow,"enabled":enabled,"token":token}))
    }
    pub async fn event(&self, id: &str, token: &str, key: &str, input: Value) -> Result<Value> {
        identifier(id)?;
        ensure!(
            !key.is_empty() && key.len() <= 200,
            "Event idempotency key required"
        );
        let r = self
            .store
            .get("trigger", id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Event rejected"))?;
        let expected = r.value["digest"].as_str().unwrap_or("");
        ensure!(
            r.value["enabled"] == true
                && bool::from(digest(token).as_bytes().ct_eq(expected.as_bytes())),
            "Event rejected"
        );
        let scoped = Engine {
            store: self.store.for_origin(&r)?,
            catalog: self.catalog.clone(),
            vault: self.vault.clone(),
        };
        scoped
            .enqueue(
                r.value["workflow"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Event rejected"))?,
                &format!("event:{id}:{}", digest(key)),
                input,
            )
            .await
    }
}
#[derive(Clone)]
struct Ingress {
    template: Engine,
    slots: Arc<tokio::sync::Semaphore>,
}
pub fn router(template: Engine) -> Router {
    Router::new()
        .route("/automation/events/{tenant}/{trigger}", post(receive))
        .layer(DefaultBodyLimit::max(65536))
        .route_layer(axum::middleware::from_fn(crate::hooks::external_guard))
        .with_state(Ingress {
            template,
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        })
}
async fn receive(
    State(s): State<Ingress>,
    Path((tenant, trigger)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    let _slot = s
        .slots
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let token = headers
        .get("x-automation-token")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 200)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let store = Store::new(s.template.store.state.clone(), &tenant)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let e = Engine {
        store,
        catalog: s.template.catalog.clone(),
        vault: s.template.vault.clone(),
    };
    let input = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let r = e
        .event(&trigger, token, key, input)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(json!({"run_id":r["id"]})))
}
