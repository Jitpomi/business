use super::*;
use tower::ServiceExt;
#[tokio::test]
async fn recovery_fence_blocks_every_mutation_but_keeps_normal_read_authentication() {
    let app = Router::new()
        .fallback(|| async { StatusCode::OK })
        .layer(axum::middleware::from_fn(recovery_guard));
    for (method, path, capability, status) in [
        ("POST", "/data", Some("write"), 503),
        ("GET", "/data", Some("write"), 503),
        ("POST", "/automation", None, 503),
        ("POST", "/tenants/x/webhooks/stripe", None, 503),
        ("DELETE", "/authentication", None, 503),
        ("POST", "/data", Some("read"), 200),
        ("POST", "/authentication", None, 200),
        ("GET", "/health", None, 200),
    ] {
        let mut r = axum::http::Request::builder().method(method).uri(path);
        if let Some(c) = capability {
            r = r.header("x-service-method", c);
        }
        assert_eq!(
            app.clone()
                .oneshot(r.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
                .as_u16(),
            status,
            "{method} {path}"
        );
    }
}
