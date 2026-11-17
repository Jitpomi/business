use super::*;
#[tokio::test]
#[ignore = "Real development TypeDB, website queries, isolated plugins and durable worker; disposable database only"]
async fn website_queries_retire_portal_end_to_end() -> Result<()> {
    let _environment = environment::Environment::capture(&[
        "BUSINESS_GATEWAY_SECRET",
        "BILLING_ENABLED",
        "AUTOMATION_ENABLED",
        "AUTOMATION_MASTER_KEY_B64",
    ]);
    use server::{database, typedb::TypeDBState};
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development only"
    );
    let website = std::path::PathBuf::from(std::env::var("BUSINESS_WEBSITE_DIR")?);
    ensure!(
        website.join("portal/retirement.integration.mjs").exists(),
        "Website integration script missing"
    );
    let driver = Arc::new(
        crate::test_driver::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let name = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Disposable retirement database: {name}");
    database::install(&driver, &name).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: name.clone(),
    });
    let mut worker = None;
    let mut http_server = None;
    let result=async {
  let tenant=uuid::Uuid::new_v4().to_string();let person=uuid::Uuid::new_v4().to_string();let email="retirement-owner@example.invalid";
  let mut runtime=TenantRuntime::new(tenant.clone(),true);let bank=Arc::new(Bank{fail_detach:std::sync::atomic::AtomicBool::new(true),..Bank::default()});let mail=Arc::new(Mail::default());runtime.register("community-bank",bank.clone())?;runtime.default_financial=Some("community-bank".into());runtime.notification=Some(mail.clone());runtime.allowed_test_recipients.push("buyer@example.invalid".into());
  let engine=server::billing::engine::Engine::new(state.clone(),&tenant,Arc::new(runtime.clone()))?;
  let link=server::auth::local_link("$u",&person)?;server::auth::plugins::local::Local::configured(&name)?.import(&person,email,&bcrypt::hash("retirement-test-password",4)?,"initial")?;
  engine.write(format!("insert $t isa tenant,has tenant_id '{tenant}',has name 'Test merchant',has status 'active',has revision 'initial';$u isa person,has identity_id '{person}',has email '{email}',has name 'Owner',has status 'active';{link}(tenant:$t,member:$u) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;fetch {{'id':$u.identity_id}};")).await?;
  let mut registry=TenantRegistry::default();registry.register(runtime)?;
  std::env::set_var("BUSINESS_GATEWAY_SECRET","retirement-test-gateway-secret-at-least-32-characters");std::env::set_var("BILLING_ENABLED","true");std::env::set_var("AUTOMATION_ENABLED","true");std::env::set_var("AUTOMATION_MASTER_KEY_B64","AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=");
  let mut catalog=server::automation::connectors::Catalog::default();catalog.register(Arc::new(BankConnector{tenants:HashMap::from([(tenant.clone(),(bank.clone(),mail.clone()))])}))?;
  let(app,http)=server::app::build_with_connectors(state.clone(),"retirement-test-signing-secret-long-enough".into(),Arc::new(registry.clone()),catalog)?;
  let template:Arc<server::automation::engine::Engine>=app.get("automation.engine").unwrap();
  let router=server::app::router(http).merge(server::billing::http::managed_router(state.clone(),Arc::new(registry),Some(tenant.clone()))?);
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await?;let base=format!("http://{}",listener.local_addr()?);
  let (stop_http, stopped_http) = tokio::sync::oneshot::channel();
  http_server = Some((stop_http, tokio::spawn(async move {
      axum::serve(listener,router).with_graceful_shutdown(async {let _=stopped_http.await;}).await
  })));
  worker = Some(server::background::spawn_all((*template).clone()));
  let request=serde_json::json!({"base":base,"tenant":tenant,"email":email,"gateway":std::env::var("BUSINESS_GATEWAY_SECRET").unwrap_or_default()});
  let output=tokio::task::spawn_blocking(move||->Result<std::process::Output>{
   use std::io::Write;
   let mut child=std::process::Command::new("node").arg(website.join("portal/retirement.integration.mjs")).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn()?;
   child.stdin.take().unwrap().write_all(request.to_string().as_bytes())?;Ok(child.wait_with_output()?)
  }).await??;
  eprintln!("{}",String::from_utf8_lossy(&output.stderr));
  ensure!(output.status.success(),"Website retirement test failed: {}",String::from_utf8_lossy(&output.stdout));
  ensure!(bank.attempts.lock().unwrap().values().all(|n|*n==1),"Collection was repeated");
  Ok::<_,anyhow::Error>(())
 }.await;
    if let Some((stop, server)) = http_server {
        let _ = stop.send(());
        server.await??;
    }
    if let Some(worker) = worker {
        worker.shutdown().await?;
    }
    if result.is_ok() {
        driver.databases().get(&name).await?.delete().await?;
    } else {
        eprintln!("Retained retirement fixture: {name}");
    }
    result
}
