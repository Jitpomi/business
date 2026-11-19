//! Synthetic authenticated data workload; never loads a payment plugin or customer database.
use super::*;
use anyhow::{ensure, Result};
use futures::{stream, StreamExt, TryStreamExt};
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};

#[tokio::test]
#[ignore = "Explicit isolated capacity rehearsal; no provider calls; not a production hardware certification"]
async fn authenticated_native_reads_remain_isolated_under_capacity_load() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    super::support::auth()?;
    for (key, value) in [
        ("AUTOMATION_ENABLED", "false"),
        ("BILLING_ENABLED", "false"),
        ("OPERATIONS_ENABLED", "false"),
        ("BUSINESS_ACTIVE_REQUESTS", "3"),
    ] {
        std::env::set_var(key, value);
    }
    std::env::remove_var("BUSINESS_GATEWAY_SECRET");
    let driver = Arc::new(
        crate::test_driver::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let database = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    server::database::install(&driver, &database).await?;
    let result = async {
        let state = Arc::new(server::typedb::TypeDBState { driver: driver.clone(), database: database.clone() });
        dog_typedb::TypeDBAdapter::new(state.clone()).write(json!({"query":include_str!("fixtures.tql")})).await?;
        super::seed_accounts(&database)?;
        let (app, http) = build_with_state(state, "capacity-synthetic-secret-at-least-32-bytes".into())?;
        let routes = server::app::runtime_router(&app, http).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        struct Running(tokio::task::JoinHandle<()>);
        impl Drop for Running { fn drop(&mut self) { self.0.abort(); } }
        let _running = Running(tokio::spawn(async move { axum::serve(listener, routes).await.unwrap(); }));
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build()?;
        let alice = schema_policy_tests::login(&client, &base, "tenant-a", "alice@example.com").await?;
        let dave = schema_policy_tests::login(&client, &base, "tenant-b", "dave@example.com").await?;
        let started = Instant::now();
        let durations: Vec<u128> = stream::iter(0..120).map(|i| {
            let (client, base) = (&client, &base);
            let user = if i % 2 == 0 { &alice } else { &dave };
            async move {
                let (tenant, expected) = if i % 2 == 0 { ("tenant-a", "files") } else { ("tenant-b", "foreign") };
                let at = Instant::now();
                let response = client.post(format!("{base}/data")).header("x-tenant-id", tenant).header("x-service-method", "read").bearer_auth(user["accessToken"].as_str().unwrap()).json(&json!({"query":"match $r isa resource; fetch {'id':$r.resource_id};"})).send().await?;
                ensure!(response.status().is_success(), "Capacity request failed: {}", response.status());
                let value: Value = response.json().await?;
                let rows = value.pointer("/ok/answers").and_then(Value::as_array).ok_or_else(|| anyhow::anyhow!("Missing scoped answers"))?;
                ensure!(rows.len() == 1 && rows[0]["data"]["id"] == expected, "Native tenant isolation failed under load");
                Ok::<_, anyhow::Error>(at.elapsed().as_millis())
            }
        }).buffer_unordered(3).try_collect().await?;
        let mut sorted = durations; sorted.sort_unstable();
        let p95 = sorted[113]; let p99 = sorted[118];
        println!("{}", json!({"workload":"synthetic authenticated native resource reads", "requests":120,"concurrency":3,"p95_ms":p95,"p99_ms":p99,"duration_ms":started.elapsed().as_millis(),"tenant_isolation_verified":true,"provider_calls":0,"production_hardware_verified":false}));
        ensure!(p95 <= 5000 && p99 <= 15000, "Initial capacity latency target missed");
        Ok::<_, anyhow::Error>(())
    }.await;
    if result.is_ok() {
        driver.databases().get(&database).await?.delete().await?;
    } else {
        eprintln!("Capacity fixture retained: {database}");
    }
    result
}
