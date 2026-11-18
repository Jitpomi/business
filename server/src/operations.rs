//! Read-only operational observations. Native tenant authorization and financial policies are unchanged.
use crate::typedb::TypeDBState;
use anyhow::{ensure, Result};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use subtle::ConstantTimeEq;

pub fn recovery_mode() -> Result<bool> {
    match std::env::var("BUSINESS_RECOVERY_MODE").as_deref() {
        Ok("true") => {
            let db = std::env::var("TYPEDB_DB")?;
            let suffix = db.strip_prefix("business_backend_test_").ok_or_else(|| {
                anyhow::anyhow!("Recovery mode requires an explicit generated shadow database")
            })?;
            uuid::Uuid::parse_str(suffix)?;
            Ok(true)
        }
        Ok("false") | Err(std::env::VarError::NotPresent) => Ok(false),
        _ => anyhow::bail!("BUSINESS_RECOVERY_MODE must be true or false"),
    }
}
/// An additional deployment safety fence; every allowed read still runs its ordinary dog-auth/TypeDB policies.
pub async fn recovery_guard(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let method = request.method();
    let capability = request
        .headers()
        .get("x-service-method")
        .and_then(|v| v.to_str().ok());
    let allowed = (method == Method::GET && capability.is_none_or(|v| v == "find" || v == "read"))
        || (method == Method::POST && path == "/data" && capability == Some("read"))
        || (method == Method::POST && path == "/authentication" && capability.is_none());
    if !allowed {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"message":"Recovery validation is read-only; writes and payment execution are disabled"}))).into_response();
    }
    next.run(request).await
}
#[derive(Clone)]
struct Monitor {
    state: Arc<TypeDBState>,
    token: Arc<String>,
    vault: crate::automation::vault::Vault,
    canary: Arc<String>,
}
const CANARY_CONTEXT: &str = "business/operations/readiness/v1";
pub fn router(state: Arc<TypeDBState>) -> Result<Router> {
    let token = std::env::var("OPERATIONS_READ_TOKEN")?;
    ensure!(
        (32..=256).contains(&token.len())
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Operations token must be a strong opaque token"
    );
    let canary = std::env::var("OPERATIONS_VAULT_CANARY_REF")?;
    uuid::Uuid::parse_str(canary.strip_prefix("secret:").ok_or_else(|| {
        anyhow::anyhow!("Operations canary must be an external secret reference")
    })?)?;
    let monitor = Monitor {
        state,
        token: Arc::new(token),
        vault: crate::automation::vault::Vault::from_env()?,
        canary: Arc::new(canary),
    };
    Ok(Router::new()
        .route("/health/ready", axum::routing::get(readiness))
        .with_state(monitor)
        .route_layer(axum::middleware::from_fn(crate::hooks::external_guard)))
}
pub async fn seed_canary() -> Result<String> {
    crate::automation::vault::Vault::from_env()?
        .seal(
            CANARY_CONTEXT,
            &json!({"purpose":"operations-readiness","version":1}),
        )
        .await
}
async fn count(tx: &typedb_driver::Transaction, pattern: &str) -> Result<u64> {
    let rows = crate::database::documents(
        tx,
        &format!("match {pattern} reduce $n=count;fetch {{'count':$n}};"),
    )
    .await?;
    rows.first()
        .and_then(|v| v["count"].as_u64())
        .ok_or_else(|| anyhow::anyhow!("Invalid operational aggregate"))
}
/// Fixed aggregate queries expose neither tenant identifiers nor business/credential payloads.
pub async fn metrics(state: &TypeDBState) -> Result<Value> {
    let tx = state
        .driver
        .transaction(&state.database, typedb_driver::TransactionType::Read)
        .await?;
    let stale_job = crate::clock::unix_seconds() - 900;
    let stale_invoice = crate::clock::unix_seconds() - 3600;
    let failed = count(
        &tx,
        "$r isa automation_record,has auto_kind 'job',has auto_status 'failed';",
    )
    .await?;
    let overdue=count(&tx,&format!("$r isa automation_record,has auto_kind 'job',has auto_status $status,has auto_when $due;$due<{stale_job};{{$status=='enqueued';}} or {{$status=='retrying';}} or {{$status=='processing';}};")).await?;
    let attention = count(
        &tx,
        "$r isa automation_record,has auto_kind 'run',has auto_status 'attention';",
    )
    .await?;
    let failed_runs = count(
        &tx,
        "$r isa automation_record,has auto_kind 'run',has auto_status 'failed';",
    )
    .await?;
    let invoice_attention = count(&tx, "$i isa bill_invoice,has bill_status 'attention';").await?;
    let stale=count(&tx,&format!("$i isa bill_invoice,has bill_status $status,has bill_updated $updated;$updated<{stale_invoice};{{$status=='open';}} or {{$status=='processing';}};(plan:$p,invoice:$i) isa bill_invoice_owner;$p has bill_status 'active';not {{(invoice:$i,notice:$archive) isa bill_notice_owner;$archive has bill_status 'imported-history';}};")).await?;
    tx.close().await?;
    Ok(
        json!({"failed_jobs":failed,"failed_runs":failed_runs,"overdue_jobs":overdue,"attention_runs":attention,"attention_invoices":invoice_attention,"stale_reconciliations":stale}),
    )
}
async fn readiness(State(m): State<Monitor>, headers: HeaderMap) -> Response {
    let expected = format!("Bearer {}", m.token);
    let supplied = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !bool::from(expected.as_bytes().ct_eq(supplied.as_bytes())) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(25), async {
        let metrics = metrics(&m.state).await?;
        let canary = m.vault.open(CANARY_CONTEXT, &m.canary).await?;
        ensure!(
            canary == json!({"purpose":"operations-readiness","version":1}),
            "Invalid canary"
        );
        Ok::<_, anyhow::Error>(metrics)
    })
    .await;
    let (status, body) = match result {
        Ok(Ok(metrics)) => (
            StatusCode::OK,
            json!({"version":1,"dependencies":"ready","metrics":metrics}),
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"version":1,"dependencies":"unavailable"}),
        ),
    };
    (status, [("cache-control", "no-store")], Json(body)).into_response()
}
/// Explicit, graph-preserving development update of this release's single control function.
pub async fn refresh_control_policy(
    driver: &typedb_driver::TypeDBDriver,
    name: &str,
    checkpoint: &std::path::Path,
) -> Result<()> {
    crate::database::target(name)?;
    ensure!(
        crate::database::development_version(name).is_some()
            || name.starts_with("business_backend_test_"),
        "Development database required"
    );
    ensure!(
        std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
        "Quiesce writers before native policy update"
    );
    let expected: std::collections::BTreeMap<String, usize> =
        serde_json::from_slice(&std::fs::read(checkpoint)?)?;
    ensure!(
        crate::database::snapshot::graph(driver, name, &[]).await? == expected,
        "Checkpoint is stale; no policy changed"
    );
    let body = crate::database::BUSINESS_RULES
        .split("fun automation_control(")
        .nth(1)
        .and_then(|s| s.split("fun automation_claim_allowed(").next())
        .ok_or_else(|| anyhow::anyhow!("Canonical control policy missing"))?;
    let tx = driver
        .transaction(name, typedb_driver::TransactionType::Schema)
        .await?;
    tx.query(format!("redefine fun automation_control({body}"))
        .await?;
    tx.commit().await?;
    ensure!(
        crate::database::snapshot::graph(driver, name, &[]).await? == expected,
        "Graph changed during policy update; retain checkpoint and investigate"
    );
    crate::database::verify_runtime(driver, name).await?;
    Ok(())
}
#[cfg(test)]
#[path = "../tests/unit/operations.rs"]
mod tests;
