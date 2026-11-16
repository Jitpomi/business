use anyhow::{ensure, Result};
use serde_json::{json, Value};
use server::automation::{
    connectors::{redact, Catalog, Connector, Http, Invocation},
    engine::Engine,
    model::{ActionSpec, Effect, Run, Schedule, Step, Workflow},
    store::Store,
    vault::Vault,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
#[tokio::test]
async fn vault_binds_tenant_version_and_detects_tampering() -> Result<()> {
    let vault = Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")?;
    let secret = json!({"token":"private-token"});
    let a = vault.seal("tenant-a/one/v1", &secret).await?;
    let b = vault.seal("tenant-a/one/v1", &secret).await?;
    ensure!(a != b, "Nonce reused");
    ensure!(!a.contains("private-token"));
    ensure!(vault.open("tenant-a/one/v1", &a).await? == secret);
    for context in ["tenant-b/one/v1", "tenant-a/one/v2"] {
        ensure!(vault.open(context, &a).await.is_err());
    }
    ensure!(Vault::default().seal("x", &secret).await.is_err());
    let mut damaged = a.into_bytes();
    damaged[20] = if damaged[20] == b'A' { b'B' } else { b'A' };
    ensure!(vault
        .open("tenant-a/one/v1", std::str::from_utf8(&damaged)?)
        .await
        .is_err());
    Ok(())
}
#[test]
fn http_rejects_private_destinations_and_embedded_credentials() -> Result<()> {
    for ip in [
        "127.0.0.1",
        "10.1.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "2002:7f00:1::",
        "2001:db8::1",
    ] {
        ensure!(!Http::public(ip.parse()?), "Private address allowed: {ip}");
    }
    for ip in ["8.8.8.8", "2606:4700::1111"] {
        ensure!(Http::public(ip.parse()?));
    }
    for origin in [
        "http://example.com",
        "https://user:password@example.com",
        "https://example.com/api",
        "https://example.com:444",
    ] {
        ensure!(Http
            .validate(&json!({"origin":origin}), &Value::Null)
            .is_err());
    }
    let cleaned = redact(
        json!({"secret-token":"prefix secret-token"}),
        &json!({"token":"secret-token"}),
    );
    ensure!(!cleaned.to_string().contains("secret-token"));
    Ok(())
}
struct Recorder {
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl Connector for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }
    fn actions(&self) -> Vec<ActionSpec> {
        vec![
            ActionSpec {
                name: "send".into(),
                effect: Effect::AtMostOnce,
            },
            ActionSpec {
                name: "read".into(),
                effect: Effect::Read,
            },
        ]
    }
    fn validate(&self, _: &Value, _: &Value) -> Result<()> {
        Ok(())
    }
    async fn verify(&self, _: &Value, _: &Value) -> Result<()> {
        Ok(())
    }
    async fn execute(&self, ctx: &Invocation, action: &str, input: Value) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if input["uncertain"] == true {
            anyhow::bail!("Synthetic lost acknowledgement");
        }
        Ok(
            json!({"tenant":ctx.tenant,"key":ctx.key,"input":input,"credential":ctx.secrets["token"],"action":action}),
        )
    }
}
#[tokio::test]
#[ignore = "Development credentials, disposable database only; no external payment or mail calls"]
async fn durable_workflows_preserve_tenant_boundaries_and_uncertain_writes() -> Result<()> {
    let _environment =
        environment::Environment::capture(&["AUTOMATION_ENABLED", "AUTOMATION_MASTER_KEY_B64"]);
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    let result=async{
 server::database::install(&driver,&db).await?;
 let state=Arc::new(server::typedb::TypeDBState{driver:driver.clone(),database:db.clone()});
 let owner=uuid::Uuid::new_v4().to_string();let tenant=uuid::Uuid::new_v4().to_string();let other=uuid::Uuid::new_v4().to_string();
 let password_hash=bcrypt::hash("automation-owner-test-password",4)?;let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;let auth_link_0=server::auth::local_link("$u",&owner.to_string())?;server::auth::plugins::local::Local::configured(&db)?.import(&owner.to_string(),"automation@example.invalid",&password_hash.to_string(),"initial")?;
tx.query(format!(r#"insert $t isa tenant,has tenant_id "{tenant}",has name "Automation test",has status "active",has revision "initial";$b isa tenant,has tenant_id "{other}",has name "Other tenant",has status "active",has revision "initial";$u isa person,has identity_id "{owner}",has email "automation@example.invalid",has name "Owner",has status "active"; {auth_link_0}(tenant:$t,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;"#)).await?;tx.commit().await?;
 let recorder=Arc::new(Recorder{calls:AtomicUsize::new(0)});let mut catalog=Catalog::default();catalog.register(recorder.clone())?;
 let e=Engine{store:Store::new(state.clone(),&tenant)?,catalog:Arc::new(catalog),vault:Vault::with_store("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",server::automation::secret_store::Files::configured()?)?};
 let public=e.connect("crm","recorder",json!({}),json!({"token":"DO-NOT-LEAK"})).await?;ensure!(!public.to_string().contains("secret_ref")&&!public.to_string().contains("DO-NOT-LEAK"));e.verify_connection("crm").await?;
 let b=Store::new(state.clone(),&other)?;ensure!(b.get("connection","crm").await?.is_none());ensure!(b.owner(&owner).await.is_err());
 let page=e.store.page("connection",None,1).await?;ensure!(page.len()==1);let cursor=page[0]["key"].as_str().unwrap();ensure!(e.store.page("connection",Some(cursor),1).await?.is_empty());ensure!(b.page("connection",Some(cursor),1).await.is_err(),"Pagination accepted another tenant's cursor");
 let w=Workflow{id:"onboarding".into(),revision:String::new(),name:"Onboarding".into(),enabled:true,steps:vec![Step::Transform{id:"mapped".into(),value:json!({"customer":{"$ref":"input/customer"}})},Step::Approval{id:"review".into()},Step::Action{id:"deliver".into(),connection:"crm".into(),action:"send".into(),input:json!({"$ref":"steps/mapped"})},Step::Delay{id:"pause".into(),seconds:0},Step::Transform{id:"done".into(),value:json!(true)}]};
 let published=e.publish(w.clone(),&owner).await?;let original_revision=published["workflow"]["revision"].as_str().unwrap().to_owned();ensure!(e.publish(w.clone(),&owner).await.is_err(),"Stale publication overwrote a workflow");ensure!(e.store.get("workflow_version",&original_revision).await?.is_some(),"Published definition was lost");let mut updated:Workflow=serde_json::from_value(published["workflow"].clone())?;updated.name="Updated onboarding".into();let updated=e.publish(updated,&owner).await?;ensure!(updated["workflow"]["revision"]!=original_revision&&e.store.get("workflow_version",&original_revision).await?.unwrap().value["workflow"]["name"]=="Onboarding","Prior workflow version changed");let run=e.enqueue(&w.id,"event-1",json!({"customer":"Jane"})).await?;let id=run["id"].as_str().unwrap();ensure!(e.enqueue(&w.id,"event-1",json!({"customer":"Jane"})).await?["id"]==id);ensure!(e.enqueue(&w.id,"event-1",json!({"customer":"Changed"})).await.is_err());
 ensure!(!e.store.due().await?.is_empty());e.advance(id).await?;ensure!(e.advance(id).await?["status"]=="approval");ensure!(recorder.calls.load(Ordering::SeqCst)==0);e.approve(id,&owner).await?;
 // Reconstructing the engine simulates a worker restart; persisted state is authoritative.
 let restarted=e.clone();for _ in 0..5{restarted.advance(id).await?;}let completed=e.store.get("run",id).await?.unwrap().value;ensure!(completed["status"]=="completed","{completed}");ensure!(!completed.to_string().contains("DO-NOT-LEAK"));ensure!(recorder.calls.load(Ordering::SeqCst)==1);
 let mut uncertain=w.clone();uncertain.id="uncertain".into();uncertain.steps=vec![Step::Action{id:"write".into(),connection:"crm".into(),action:"send".into(),input:json!({"uncertain":true})}];e.publish(uncertain.clone(),&owner).await?;let r=e.enqueue(&uncertain.id,"one",Value::Null).await?;let rid=r["id"].as_str().unwrap();ensure!(e.advance(rid).await?["status"]=="attention");e.advance(rid).await?;ensure!(recorder.calls.load(Ordering::SeqCst)==2);e.resolve(rid,&owner,"Checked the external system; action succeeded",json!({"id":"verified"})).await?;ensure!(e.advance(rid).await?["status"]=="completed");ensure!(recorder.calls.load(Ordering::SeqCst)==2);
 let mut sw=w;sw.id="scheduled".into();sw.steps=vec![Step::Transform{id:"one".into(),value:json!(true)}];e.publish(sw,&owner).await?;e.schedule(Schedule{id:"daily".into(),workflow:"scheduled".into(),every_seconds:60,next:server::automation::engine::now(),sequence:999,input:Value::Null,enabled:true}).await?;e.fire_schedule("daily").await?;e.fire_schedule("daily").await?;ensure!(e.store.get("schedule","daily").await?.unwrap().value["sequence"]==1);

 let literal=Workflow{id:"literal-data".into(),revision:String::new(),name:"Keep customer data literal".into(),enabled:true,steps:vec![Step::Transform{id:"copy".into(),value:json!({"$ref":"input/customer"})}]};e.publish(literal,&owner).await?;let literal=e.enqueue("literal-data","literal",json!({"customer":{"$ref":"steps/never-evaluate"}})).await?;let copied=e.advance(literal["id"].as_str().unwrap()).await?;ensure!(copied["status"]=="completed"&&copied["outputs"]["copy"]==json!({"$ref":"steps/never-evaluate"}),"Customer data was evaluated as a template");
 let concurrent=Workflow{id:"concurrent".into(),revision:String::new(),name:"Concurrent claim".into(),enabled:true,steps:vec![Step::Action{id:"once".into(),connection:"crm".into(),action:"send".into(),input:Value::Null}]};e.publish(concurrent,&owner).await?;let run_once=e.enqueue("concurrent","race",Value::Null).await?;let once=run_once["id"].as_str().unwrap();let before=recorder.calls.load(Ordering::SeqCst);let(a,b)=tokio::join!(e.advance(once),e.advance(once));ensure!(a.is_ok()||b.is_ok());ensure!(recorder.calls.load(Ordering::SeqCst)==before+1,"Concurrent workers duplicated a write");
 let mut disabled:Run=serde_json::from_value(completed)?;disabled.id="disconnected".into();disabled.cursor=2;disabled.status="queued".into();disabled.wake=0;e.store.insert("run",&disabled.id,&serde_json::to_value(&disabled)?,"queued",0).await?;e.disconnect("crm").await?;ensure!(e.advance("disconnected").await?["status"]=="blocked");ensure!(recorder.calls.load(Ordering::SeqCst)==3);

 // A shared-secret trigger cannot cross tenants or replay different payloads.
 let trigger=e.trigger("crm-events","scheduled",true).await?;let token=trigger["token"].as_str().unwrap();ensure!(e.event("crm-events","wrong","event",Value::Null).await.is_err());
 let event=e.event("crm-events",token,"event",json!({"event":1})).await?;ensure!(e.event("crm-events",token,"event",json!({"event":1})).await?["id"]==event["id"]);ensure!(e.event("crm-events",token,"event",json!({"event":2})).await.is_err());
 let driven=e.enqueue("scheduled","worker-driven",Value::Null).await?;let driven_id=driven["id"].as_str().unwrap().to_owned();let worker=server::automation::runtime::spawn_all(e.clone());let outcome=tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{if e.store.get("run",&driven_id).await?.unwrap().value["status"]=="completed"{break Ok::<_,anyhow::Error>(());}tokio::time::sleep(std::time::Duration::from_millis(500)).await;}}).await;worker.shutdown().await?;outcome??;
 // Real authentication and schema-policy ingress must not expose or forge records.
 std::env::set_var("AUTOMATION_ENABLED","true");std::env::set_var("AUTOMATION_MASTER_KEY_B64","AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
 let (_,http)=server::app::build_with_plugins(state.clone(),"automation-test-signing-secret-with-at-least-32-bytes".into(),Arc::new(server::finance::TenantRegistry::default()))?;
 let router=server::app::router(http);ensure!(call(&router,&tenant,"","/automation",None).await?.0>=400);
 let(status,login)=call(&router,&tenant,"","/authentication",Some(json!({"strategy":"local","email":"automation@example.invalid","password":"automation-owner-test-password"}))).await?;ensure!(status==200,"Login failed: {status} {login}");let jwt=login["accessToken"].as_str().unwrap();
 let(status,listing)=call(&router,&tenant,jwt,"/automation",None).await?;ensure!(status==200,"Automation listing failed: {listing}");ensure!(!listing.to_string().contains("secret_ref")&&!listing.to_string().contains("DO-NOT-LEAK"));ensure!(call(&router,&other,jwt,"/automation",None).await?.0>=400);
 let(status,data)=call(&router,&tenant,jwt,"/data",Some(json!({"query":"match $r isa automation_record; fetch {'secret':$r.auto_payload};"}))).await?;ensure!(status>=400||(!data.to_string().contains("DO-NOT-LEAK")&&!data.to_string().contains("secret_ref")),"Generic data exposed automation credentials");
 let forged=format!("insert $r isa automation_record,has auto_key \"{tenant}/run/forged\",has auto_tenant \"{tenant}\",has auto_kind \"run\",has auto_payload \"{{}}\",has auto_revision \"forged\",has auto_status \"queued\",has auto_when 0;fetch {{'key':$r.auto_key}};");ensure!(call(&router,&tenant,jwt,"/data",Some(json!({"query":forged}))).await?.0>=400,"Generic data forged a run");ensure!(e.store.get("run","forged").await?.is_none());
 let(status,_)=call(&router,&tenant,jwt,"/automation",Some(json!({"action":"connect","id":"http-api","connector":"http","config":{"origin":"https://example.com"},"secrets":{"bearer":"PRIVATE-HTTP-TOKEN"},"tenant_id":other}))).await?;ensure!(status>=400,"Body tenant override accepted");
 let(status,connected)=call(&router,&tenant,jwt,"/automation",Some(json!({"action":"connect","id":"owner-http","connector":"http","config":{"origin":"https://example.com"},"secrets":{"bearer":"PRIVATE-HTTP-TOKEN"}}))).await?;ensure!(status<300,"Owner connection failed: {connected}");ensure!(!connected.to_string().contains("PRIVATE-HTTP-TOKEN")&&!connected.to_string().contains("secret_ref"));
 let revoked=e.enqueue("scheduled","revoked-owner",Value::Null).await?;let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;tx.query(format!("match $u isa person,has identity_id \"{owner}\";$t isa tenant,has tenant_id \"{tenant}\";$m isa membership,links(tenant:$t,member:$u);update $m has membership_role \"member\";")).await?;tx.commit().await?;ensure!(e.advance(revoked["id"].as_str().unwrap()).await?["status"]=="blocked");ensure!(call(&router,&tenant,jwt,"/automation",None).await?.0>=400,"Revoked owner retained automation access");

 Ok::<_,anyhow::Error>(())
 }.await;
    if driver.databases().contains(&db).await? {
        driver.databases().get(&db).await?.delete().await?;
    }
    result
}

async fn call(
    router: &axum::Router,
    tenant: &str,
    token: &str,
    path: &str,
    data: Option<Value>,
) -> Result<(u16, Value)> {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .uri(path)
        .method(if data.is_some() { "POST" } else { "GET" })
        .header("x-tenant-id", tenant)
        .header("content-type", "application/json");
    if path == "/data" {
        request = request.header(
            "x-service-method",
            if data
                .as_ref()
                .and_then(|v| v["query"].as_str())
                .is_some_and(|q| q.starts_with("insert"))
            {
                "write"
            } else {
                "read"
            },
        );
    }
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(axum::body::Body::from(
            data.map(|v| v.to_string()).unwrap_or_default(),
        ))?)
        .await?;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 2000000).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}
#[test]
fn mappings_are_bounded_and_never_evaluate_customer_data() -> Result<()> {
    use std::collections::BTreeMap;
    let input = json!({"customer":{"$ref":"steps/secret"}});
    let step = Step::Transform {
        id: "map".into(),
        value: json!({"$ref":"input/customer"}),
    };
    match step.resolve(&input, &BTreeMap::new())? {
        Step::Transform { value, .. } => ensure!(value == input["customer"]),
        _ => unreachable!(),
    };
    let mut outputs = BTreeMap::new();
    outputs.insert("large".into(), json!("x".repeat(200000)));
    ensure!(server::automation::model::bind(
        &json!([{"$ref":"steps/large"},{"$ref":"steps/large"}]),
        &Value::Null,
        &outputs
    )
    .is_err());
    let workflow = Workflow {
        id: "bad".into(),
        revision: String::new(),
        name: "Invalid future reference".into(),
        enabled: true,
        steps: vec![
            Step::Transform {
                id: "first".into(),
                value: json!({"$ref":"steps/later"}),
            },
            Step::Transform {
                id: "later".into(),
                value: Value::Null,
            },
        ],
    };
    ensure!(workflow.validate().is_err());
    Ok(())
}
#[test]
fn builtin_connections_validate_without_fabricating_proxy_identities() -> Result<()> {
    use server::automation::builtin::Builtin;
    Builtin("stripe").validate(
        &json!({"sandbox":true}),
        &json!({"token":"sk_test_synthetic_valid_key"}),
    )?;
    ensure!(Builtin("stripe")
        .validate(
            &json!({"sandbox":false}),
            &json!({"token":"sk_test_synthetic_valid_key"})
        )
        .is_err());
    Builtin("mercury").validate(
        &json!({"sandbox":true,"destination":uuid::Uuid::new_v4().to_string()}),
        &json!({"token":"synthetic-mercury-token"}),
    )?;
    Builtin("resend").validate(
        &json!({"sandbox":true,"from":"Test <test@example.invalid>"}),
        &json!({"token":"re_synthetic_key"}),
    )?;
    ensure!(Builtin("resend")
        .validate_input(
            &json!({"sandbox":true,"allowed_test_recipients":["approved@example.invalid"]}),
            "send",
            &json!({"to":"other@example.invalid","subject":"Test","text":"Body"})
        )
        .is_err());
    ensure!(Http
        .validate(
            &json!({"origin":"https://example.com"}),
            &json!({"bearer":false})
        )
        .is_err());
    Ok(())
}
#[tokio::test]
#[ignore = "Development TypeDB, real backup and installer binaries against a disposable database"]
async fn additive_install_checks_backup_and_preserves_existing_work() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    driver.databases().create(&db).await?;
    let result = async {
        let tx = driver
            .transaction(&db, typedb_driver::TransactionType::Schema)
            .await?;
        for source in [
            server::database::SCHEMA,
            server::database::FUNCTIONS,
            server::database::BILLING,
            server::database::SCHEMA_POLICY,
            server::database::ACCOUNT_LINKS,
        ] {
            tx.query(source).await?;
        }
        tx.commit().await?;
        let (tenant, owner) = server::database::bootstrap(
            &driver,
            &db,
            server::database::Owner {
                email: "migration-owner@example.invalid".into(),
                name: "Owner".into(),
                tenant_name: "Original work".into(),
                password: "migration-test-password".into(),
            },
        )
        .await?;
        let before = server::database::snapshot::records(&driver, &db).await?;
        let backup = tempfile::tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(backup.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_database_backup"))
            .args(["export", &db, backup.path().to_str().unwrap()])
            .output()?;
        ensure!(
            out.status.success(),
            "Backup failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let install = || {
            let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_automation"));
            c.args(["install", &db])
                .env("AUTOMATION_BACKUP_DIR", backup.path())
                .env("AUTOMATION_WRITERS_STOPPED", "true");
            c
        };
        ensure!(
            !install()
                .env_remove("AUTOMATION_WRITERS_STOPPED")
                .output()?
                .status
                .success(),
            "Installer ignored active writers"
        );
        let out = install().output()?;
        ensure!(
            out.status.success(),
            "Additive install failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        ensure!(
            server::database::snapshot::records(&driver, &db).await? == before,
            "Install changed original work"
        );
        ensure!(
            install().output()?.status.success(),
            "Repeated install failed"
        );
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_automation"))
            .args(["import", &db])
            .env("AUTOMATION_BACKUP_DIR", backup.path())
            .env("AUTOMATION_WRITERS_STOPPED", "true")
            .env("AUTOMATION_IMPORT_TENANT", &tenant)
            .env("AUTOMATION_IMPORT_OWNER", &owner)
            .env(
                "AUTOMATION_MASTER_KEY_B64",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            )
            .output()?;
        ensure!(
            out.status.success(),
            "Import failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let state = Arc::new(server::typedb::TypeDBState {
            driver: driver.clone(),
            database: db.clone(),
        });
        ensure!(
            Store::new(state, &tenant)?
                .get("schedule", "billing-cycle")
                .await?
                .is_some(),
            "Imported schedule missing"
        );
        let tx = driver
            .transaction(&db, typedb_driver::TransactionType::Write)
            .await?;
        tx.query("match $t isa tenant;update $t has name 'Changed after backup';")
            .await?;
        tx.commit().await?;
        ensure!(
            !install().output()?.status.success(),
            "Installer accepted a stale backup"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    driver.databases().get(&db).await?.delete().await?;
    result
}

#[path = "../support/mod.rs"]
mod support;

#[path = "../support/environment.rs"]
mod environment;
