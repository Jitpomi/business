//! Shared authenticated HTTP and replay checks for real-provider acceptance.
use super::*;
use serde_json::Value;
use std::path::Path;
use tower::ServiceExt;

pub(super) fn save(path: &Path, value: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
pub(super) async fn http(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    path: &str,
    data: Value,
    custom: Option<&str>,
) -> Result<(u16, Value)> {
    let mut request = axum::http::Request::post(path)
        .header("content-type", "application/json")
        .header("x-tenant-id", tenant);
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(custom) = custom {
        request = request.header("x-service-method", custom);
    }
    let response = app
        .clone()
        .oneshot(request.body(axum::body::Body::from(data.to_string()))?)
        .await?;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1_048_576).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}
pub(super) async fn ok(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    path: &str,
    data: Value,
    custom: Option<&str>,
) -> Result<Value> {
    let (status, body) = http(app, tenant, token, path, data, custom).await?;
    ensure!(
        status == 200,
        "Sandbox acceptance request {path} failed with HTTP {status}: {body}"
    );
    Ok(body)
}
pub(super) async fn completed(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    id: &str,
) -> Result<Value> {
    // Billing actions may legitimately run for 300 seconds, followed by the
    // durable-claim margin. Do not stop the fixture worker after only 45 polls
    // while an at-most-once action is still within its execution deadline.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(420);
    while tokio::time::Instant::now() < deadline {
        let row = ok(
            app,
            tenant,
            token,
            "/automation",
            json!({"action":"inspect","kind":"run","id":id}),
            None,
        )
        .await?;
        let run = &row["data"];
        if run["status"] == "completed" {
            return Ok(run.clone());
        }
        ensure!(
            !["attention", "blocked", "cancelled", "failed"]
                .iter()
                .any(|s| run["status"] == *s),
            "Automation needs review; retained state: {row}"
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    anyhow::bail!("Automation did not finish; checkpoint retained, no blind replay")
}
pub(super) async fn event(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    workflow: &str,
    key: &str,
) -> Result<Value> {
    let row = ok(
        app,
        tenant,
        token,
        "/automation",
        json!({"action":"run","workflow":workflow,"key":key,"input":null}),
        None,
    )
    .await?;
    let id = row["id"].as_str().context("Missing run ID")?;
    let done = completed(app, tenant, token, id).await?;
    let replay = ok(
        app,
        tenant,
        token,
        "/automation",
        json!({"action":"run","workflow":workflow,"key":key,"input":null}),
        None,
    )
    .await?;
    ensure!(replay["id"] == id, "Same event produced another run");
    Ok(done)
}
