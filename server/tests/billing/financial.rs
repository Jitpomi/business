use anyhow::{ensure, Result};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use serde_json::{json, Value};
use server::billing::engine::Engine;
use tower::ServiceExt;
async fn call(
    router: &axum::Router,
    tenant: &str,
    token: &str,
    path: &str,
    data: Option<Value>,
) -> Result<(u16, Value)> {
    let mut request = Request::builder()
        .uri(path)
        .method(if data.is_some() { "POST" } else { "GET" })
        .header("x-tenant-id", tenant)
        .header("content-type", "application/json");
    if path == "/data" {
        request = request.header("x-service-method", "read");
    }
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(data.map(|d| d.to_string()).unwrap_or_default()))?)
        .await?;
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1_000_000).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}
pub async fn check(engine: &Engine) -> Result<()> {
    std::env::set_var("BILLING_ENABLED", "true");
    let customers=engine.read(format!("{} (merchant:$t,buyer:$buyer,customer:$c) isa bill_account;fetch {{'id':$c.bill_key,'buyer':$buyer.tenant_id}};",engine.prefix())).await?;
    let user = uuid::Uuid::new_v4().to_string();
    let buyer = customers[0]["buyer"].as_str().unwrap();
    let customer = customers[0]["id"].as_str().unwrap();
    let other = customers[1]["id"].as_str().unwrap();
    let hash = bcrypt::hash("portal-sandbox-test-password", 4)?;
    let auth_link_0 = server::auth::local_link("$u", &user.to_string())?;
    server::auth::plugins::local::Local::configured(&engine.state.database)?.import(
        &user.to_string(),
        "portal@example.com",
        &hash.to_string(),
        "fixture",
    )?;
    engine.write(format!(r#"{} $b isa tenant,has tenant_id "{buyer}";insert $u isa person,has identity_id "{user}",has name "TEST portal buyer",has email "portal@example.com",has status "active"; {auth_link_0}(tenant:$b,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;fetch {{"id":$u.identity_id}};"#,engine.prefix())).await?;
    let (_, http) = server::app::build_with_state(
        engine.state.clone(),
        "portal-sandbox-test-signing-secret-long-enough".into(),
    )?;
    let router = server::app::router(http);
    ensure!(
        account_profile(&router, buyer, "").await?.0 == 401,
        "Anonymous portal allowed"
    );
    let (status,login)=call(&router,"account","","/authentication",Some(json!({"strategy":"account","email":"portal@example.com","password":"portal-sandbox-test-password"}))).await?;
    ensure!(status == 200, "Portal login failed {status}");
    let token = login["accessToken"].as_str().unwrap();
    let (status, profile) = account_profile(&router, buyer, token).await?;
    ensure!(
        status == 200
            && profile[0]["email"] == "portal@example.com"
            && profile[0]["companies"]
                .as_array()
                .is_some_and(|v| v.len() == 1),
        "Profile failed: {status} {profile}"
    );
    let (status, methods) = call(
        &router,
        buyer,
        token,
        "/financial",
        Some(json!({"action":"payment_methods","customer_id":customer})),
    )
    .await?;
    ensure!(
        status == 200 && methods["methods"].as_array().is_some_and(|v| !v.is_empty()),
        "Payment listing failed: {status} {methods}"
    );
    ensure!(
        call(
            &router,
            buyer,
            token,
            "/financial",
            Some(json!({"action":"payment_methods","customer_id":other}))
        )
        .await?
        .0 == 403,
        "Cross-customer payment access allowed"
    );
    ensure!(
        account_profile(&router, &engine.merchant, token).await?.0 >= 400,
        "Cross-tenant profile allowed"
    );
    let invoices=engine.read(format!("{} (customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key}};",engine.customer_match(customer)?)).await?;
    let (status, history) = call(
        &router,
        buyer,
        token,
        "/financial",
        Some(json!({"action":"history","invoice_id":invoices[0]["id"]})),
    )
    .await?;
    ensure!(
        status == 200 && history["supported"] == true,
        "Invoice history failed: {status} {history}"
    );
    for (asset, variable, extra) in [
        ("billing-customers", "c", ",$buyer,$seller"),
        ("billing-plans", "p", ",$c"),
        ("billing-invoices", "i", ",$c"),
    ] {
        let source = std::fs::read_to_string(format!(
            "{}/public/queries/data/{asset}.tql",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let query=source.replace("fetch {",&format!("${variable} has bill_key $cursor;select ${variable}{extra},$cursor;distinct;sort $cursor;limit 100;fetch {{"));
        let response = router
            .clone()
            .oneshot(
                Request::post("/data")
                    .header("x-tenant-id", buyer)
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-service-method", "read")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"query":query}).to_string()))?,
            )
            .await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1_000_000).await?;
        let data: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            status == 200
                && data["ok"]["answers"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty()),
            "Portal {asset} query failed: {status} {data}"
        );
    }
    let method = methods["methods"][0]["id"].as_str().unwrap();
    let current = engine.customer(customer).await?;
    let plugin = methods["methods"][0]["plugin_id"].as_str().unwrap();
    let q = |s: &str| serde_json::to_string(s).unwrap();
    let query=format!("match $c isa bill_customer,has bill_key {},has bill_revision {};update $c has bill_choice 'manual';update $c has bill_consent '';update $c has bill_revision {};update $c has bill_method_plugin {};update $c has bill_payment_method {};fetch {{'id':$c.bill_key}};",q(customer),current["revision"],q(&uuid::Uuid::new_v4().to_string()),q(plugin),q(method));
    let (status, result) = native_write(&router, buyer, token, &query).await?;
    ensure!(status == 200, "Default update failed: {status} {result}");
    let current = engine.customer(customer).await?;
    ensure!(
        current["choice"] == "manual" && current["consent"] == "",
        "Changing default retained consent"
    );
    let query=format!("match $c isa bill_customer,has bill_key {},has bill_revision {};update $c has bill_choice 'manual';update $c has bill_consent '';update $c has bill_revision {};update $c has bill_payment_method '';match $m isa bill_saved_method,has method_ref {},has method_plugin {};(customer:$c,method:$m) isa bill_method_owner;update $m has method_pending true;fetch {{'id':$c.bill_key}};",q(customer),current["revision"],q(&uuid::Uuid::new_v4().to_string()),q(method),q(plugin));
    let (status, result) = native_write(&router, buyer, token, &query).await?;
    ensure!(status == 200, "Removal intent: {status} {result}");
    let(status,result)=call(&router,buyer,token,"/financial",Some(json!({"action":"detach_method","customer_id":customer,"method_id":method,"plugin_id":plugin}))).await?;
    ensure!(status == 200, "Remove method failed: {status} {result}");
    Ok(())
}

async fn account_profile(
    router: &axum::Router,
    tenant: &str,
    token: &str,
) -> Result<(u16, serde_json::Value)> {
    let (status, body) = call(
        router,
        tenant,
        token,
        "/data",
        Some(json!({"query":include_str!("../../public/queries/data/account-profile.tql")})),
    )
    .await?;
    if status != 200 {
        return Ok((status, body));
    }
    let answers = body["ok"]["answers"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing account answers: {body}"))?;
    Ok((
        status,
        json!(answers
            .iter()
            .map(|a| a["data"].clone())
            .collect::<Vec<_>>()),
    ))
}

async fn native_write(
    router: &axum::Router,
    tenant: &str,
    token: &str,
    query: &str,
) -> Result<(u16, Value)> {
    let response = router
        .clone()
        .oneshot(
            Request::post("/data")
                .header("x-tenant-id", tenant)
                .header("authorization", format!("Bearer {token}"))
                .header("x-service-method", "write")
                .header("content-type", "application/json")
                .body(Body::from(json!({"query":query}).to_string()))?,
        )
        .await?;
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1_000_000).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}
