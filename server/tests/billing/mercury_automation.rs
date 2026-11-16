//! Explicit real-sandbox acceptance through tenant-managed connections and the
//! production HTTP service/worker. Checkpoints survive pending settlement.
use super::sandbox_support::{completed, event, http, ok, save};
use super::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[tokio::test]
#[ignore = "Approved Mercury sandbox/checkpoint and dev mailbox; no invoice creation; verifies real Processing through managed plugins and automation"]
async fn mercury_managed_processing_checkpoint() -> Result<()> {
    run("processing").await
}
#[tokio::test]
#[ignore = "Checkpointed new Mercury payment must be Processing; verifies no repeated reminder or premature receipt"]
async fn mercury_managed_new_processing_payment() -> Result<()> {
    run("processing-created").await
}

#[tokio::test]
#[ignore = "Approved Mercury sandbox and dev mailbox; creates one checkpointed USD 1 invoice and one reminder through native data and automation"]
async fn mercury_managed_create_invoice_and_reminder() -> Result<()> {
    run("create").await
}
#[tokio::test]
#[ignore = "Requires the checkpointed Mercury invoice to be Paid at the provider; sends one real test receipt and checks replay"]
async fn mercury_managed_paid_receipt_and_replay() -> Result<()> {
    run("paid").await
}
#[tokio::test]
#[ignore = "Creates one separate USD 1 sandbox invoice for the supported manual-payment case; independent reminder delivery"]
async fn mercury_managed_create_manual_payment_case() -> Result<()> {
    run("create-manual").await
}
#[tokio::test]
#[ignore = "Requires Mercury's sandbox manual-payment case to report Paid; tests actual provider Paid -> application receipt"]
async fn mercury_managed_manual_paid_receipt() -> Result<()> {
    run("paid-manual").await
}

async fn prepared(root: &Path, driver: &typedb_driver::TypeDBDriver) -> Result<Value> {
    let path = root.join("prepared-run.json");
    if path.is_file() {
        return Ok(serde_json::from_slice(&std::fs::read(path)?)?);
    }
    let checkpoint = PathBuf::from(std::env::var("MERCURY_RESUME_CHECKPOINT")?);
    let mut saved: Value = serde_json::from_slice(&std::fs::read(checkpoint.join("run.json"))?)?;
    let original = saved["database"]
        .as_str()
        .context("Missing source checkpoint database")?;
    uuid::Uuid::parse_str(
        original
            .strip_prefix("business_backend_test_")
            .context("Generated checkpoints only")?,
    )?;
    let reference = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    let target = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    let data = checkpoint.join(format!("{original}.typedb"));
    driver
        .databases()
        .import_from_file(
            &reference,
            std::fs::read_to_string(checkpoint.join(format!("{original}.tql")))?,
            &data,
        )
        .await?;
    let before = database::snapshot::graph(driver, &reference, &[]).await?;
    driver
        .databases()
        .import_from_file(&target, database::import_schema(), &data)
        .await?;
    ensure!(database::snapshot::graph(driver,&target,&[]).await?==before,"Checkpoint logical records or relation roles changed; original backup untouched, generated copies retained");
    database::verify_runtime(driver, &target).await?;
    saved["database"] = json!(target);
    saved["canonical_model"] = json!(true);
    saved["checkpoint_graph_preserved"] = json!(true);
    save(&path, &saved)?;
    driver.databases().get(&reference).await?.delete().await?;
    Ok(saved)
}
async fn run(phase: &str) -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development operator config only"
    );
    let root = PathBuf::from(std::env::var("MERCURY_TEST_ROOT")?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            root.is_absolute() && root.metadata()?.permissions().mode() & 0o077 == 0,
            "Private absolute test directory required"
        );
    }
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    for dir in ["auth", "secrets", "checkpoint"] {
        let path = root.join(dir);
        if !path.exists() {
            std::fs::create_dir(&path)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                path.metadata()?.permissions().mode() & 0o077 == 0,
                "Private test subdirectories required"
            );
        }
    }
    let key_path = root.join("vault-key");
    if !key_path.exists() {
        use aes_gcm::aead::rand_core::RngCore;
        use base64::Engine;
        let mut bytes = [0u8; 32];
        aes_gcm::aead::OsRng.fill_bytes(&mut bytes);
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        use std::io::Write;
        options.open(&key_path)?.write_all(
            base64::engine::general_purpose::STANDARD
                .encode(bytes)
                .as_bytes(),
        )?;
    }
    let mut saved = prepared(&root, &driver).await?;
    let manual = phase.ends_with("-manual");
    ensure!(
        saved["canonical_model"] == true && saved["checkpoint_graph_preserved"] == true,
        "Preservation-verified canonical checkpoint required"
    );
    let name = saved["database"]
        .as_str()
        .context("Missing test database")?
        .to_owned();
    uuid::Uuid::parse_str(
        name.strip_prefix("business_backend_test_")
            .context("Generated test database only")?,
    )?;
    let tenant = saved["tenant"]
        .as_str()
        .context("Missing tenant")?
        .to_owned();
    uuid::Uuid::parse_str(&tenant)?;
    let original = saved["invoice_id"]
        .as_str()
        .context("Missing original invoice")?
        .to_owned();
    let secrets: HashMap<String, String> =
        dotenvy::from_path_iter(std::env::var("BILLING_TEST_ENV")?)?
            .collect::<std::result::Result<_, _>>()?;
    ensure!(
        secrets.get("BILLING_TEST_EMAIL").map(String::as_str) == Some("dev@jitpomi.com"),
        "Approved dev mailbox only"
    );
    for (key, value) in [
        ("AUTOMATION_ENABLED", "true"),
        ("BILLING_ENABLED", "true"),
        ("AUTH_LOCAL_ENABLED", "true"),
        ("AUTH_OIDC_PROVIDERS_JSON", "[]"),
        ("AUTH_SERVICE_REQUIREMENTS_JSON", "{}"),
        ("STRIPE_CONNECTIONS_JSON", "[]"),
        ("MERCURY_CONNECTIONS_JSON", "[]"),
    ] {
        std::env::set_var(key, value);
    }
    std::env::remove_var("BUSINESS_GATEWAY_SECRET");
    std::env::remove_var("AUTOMATION_SECRET_STORE_URL");
    std::env::set_var("AUTH_LOCAL_STORE_DIR", root.join("auth"));
    std::env::set_var("AUTOMATION_SECRET_STORE_DIR", root.join("secrets"));
    std::env::set_var(
        "AUTOMATION_MASTER_KEY_B64",
        std::fs::read_to_string(root.join("vault-key"))?,
    );
    database::verify_runtime(&driver, &name).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: name.clone(),
    });
    let owner = saved["owner"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    saved["owner"] = json!(owner);
    save(&root.join("prepared-run.json"), &saved)?;
    let q = |s: &str| serde_json::to_string(s).unwrap();
    let tx = driver.transaction(&name, TransactionType::Write).await?;
    let people = database::documents(
        &tx,
        &format!(
            "match $p isa person,has identity_id {};fetch {{'id':$p.identity_id}};",
            q(&owner)
        ),
    )
    .await?;
    if people.is_empty() {
        let link = server::auth::local_link("$p", &owner)?;
        tx.query(format!("match $t isa tenant,has tenant_id {};insert $p isa person,has identity_id {},has email 'mercury-automation-owner@example.invalid',has name 'Synthetic Mercury acceptance owner',has status 'active';{link}(tenant:$t,member:$p) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;",q(&tenant),q(&owner))).await?;
    }
    tx.commit().await?;
    let local = server::auth::plugins::local::Local::configured(&name)?;
    if local.lookup(&owner, false)?.is_none() {
        local.import(
            &owner,
            "mercury-automation-owner@example.invalid",
            &bcrypt::hash("mercury-automation-test-password", 4)?,
            "initial",
        )?;
    }
    let (app, transport) = server::app::build_with_plugins(
        state.clone(),
        "mercury-integration-test-signing-key-at-least-32-bytes".into(),
        Arc::new(server::finance::TenantRegistry::default()),
    )?;
    let template: Arc<server::automation::engine::Engine> =
        app.get("automation.engine").context("Automation missing")?;
    let router = server::app::router(transport);
    let tenant_store = server::automation::store::Store::new(state.clone(), &tenant)?;
    let login=ok(&router,&tenant,"","/authentication",json!({"strategy":"local","email":"mercury-automation-owner@example.invalid","password":"mercury-automation-test-password"}),None).await?;
    let token = login["accessToken"]
        .as_str()
        .context("Missing authentication proof")?;
    ensure!(
        http(
            &router,
            &uuid::Uuid::new_v4().to_string(),
            token,
            "/automation",
            json!({"action":"list","kind":"run"}),
            None
        )
        .await?
        .0 == 401,
        "Tenant isolation failed"
    );
    for (id, connector, config, credential) in [
        (
            "mercury",
            "mercury",
            json!({"sandbox":true,"destination":secrets["MERCURY_SANDBOX_ACCOUNT_ID"],"card_enabled":false,"ach_debit_enabled":false}),
            json!({"token":secrets["MERCURY_SANDBOX_TOKEN"]}),
        ),
        (
            "notification",
            "resend",
            json!({"sandbox":true,"from":secrets["BILLING_EMAIL_FROM"]}),
            json!({"token":secrets["RESEND_API_KEY"]}),
        ),
        ("billing", "billing", json!({}), Value::Null),
    ] {
        if tenant_store.get("connection", id).await?.is_none() {
            ok(&router,&tenant,token,"/automation",json!({"action":"connect","id":id,"connector":connector,"config":config,"secrets":credential}),None).await?;
        }
        ok(
            &router,
            &tenant,
            token,
            "/automation",
            json!({"action":"verify","id":id}),
            None,
        )
        .await?;
    }
    ok(&router,&tenant,token,"/automation",json!({"action":"configure_billing","settings":{"sandbox":true,"default_financial":"mercury","notification":"notification","reminder_days":7,"allowed_test_recipients":["dev@jitpomi.com"]}}),None).await?;
    for (id, action) in [
        ("mercury-reconcile", "reconcile"),
        ("mercury-notify", "notify"),
        ("mercury-cycle", "tick"),
    ] {
        if tenant_store.get("workflow", id).await?.is_none() {
            ok(&router,&tenant,token,"/automation",json!({"action":"publish","workflow":{"id":id,"revision":"","name":"Mercury sandbox acceptance","enabled":true,"steps":[{"kind":"action","id":"check","connection":"billing","action":action,"input":if action=="tick"{Value::Null}else{json!({"invoice_id":{"$ref":"input/invoice_id"}})}}]}}),None).await?;
        }
    }
    let worker = server::background::spawn_all((*template).clone());
    let outcome=async {
        let read=|query:String|{let state=state.clone();async move{let tx=state.driver.transaction(&state.database,TransactionType::Read).await?;database::documents(&tx,&query).await}};
        let invoice=|id:&str|format!("match $i isa bill_invoice,has bill_key {};fetch {{'id':$i.bill_key,'external':$i.bill_external_id,'url':$i.bill_url,'status':$i.bill_status}};",q(id));
        let notices=|id:&str|format!("match $i isa bill_invoice,has bill_key {};(invoice:$i,notice:$n) isa bill_notice_owner;$n has bill_key $key;select $n,$key;sort $key;fetch {{'id':$n.bill_key,'status':$n.bill_status}};",q(id));
        let target=if phase=="processing"{original.clone()}else if phase=="create"||phase=="create-manual"{
            let plan=saved[if manual{"manual_plan"}else{"new_plan"}].as_str().map(str::to_owned).unwrap_or_else(||uuid::Uuid::new_v4().to_string());saved[if manual{"manual_plan"}else{"new_plan"}]=json!(plan);save(&root.join("prepared-run.json"),&saved)?;
            let customer=read(format!("match $i isa bill_invoice,has bill_key {};(plan:$p,invoice:$i) isa bill_invoice_owner;(customer:$c,plan:$p) isa bill_plan_owner;fetch {{'id':$c.bill_key}};",q(&original))).await?;
            let anchor=chrono::Utc::now().timestamp()-60;
            let query=format!("match $c isa bill_customer,has bill_key {};put $p isa bill_plan,has bill_key {},has name 'TEST ONLY Mercury managed plugin invoice',has bill_amount 100,has bill_due_days 0,has bill_anchor {},has bill_next {},has bill_sequence 0,has bill_interval 'once',has bill_status 'active';(customer:$c,plan:$p) isa bill_plan_owner;fetch {{'id':$p.bill_key}};",q(customer[0]["id"].as_str().unwrap()),q(&plan),anchor,anchor);
            let existing=read(format!("match $p isa bill_plan,has bill_key {};fetch {{'id':$p.bill_key}};",q(&plan))).await?;
            if existing.is_empty(){let answer=ok(&router,&tenant,token,"/data",json!({"query":query}),Some("write")).await?;ensure!(answer["ok"]["answers"].as_array().is_some_and(|a|a.len()==1),"Native plan creation did not commit");}
            event(&router,&tenant,token,"mercury-cycle",if manual{"create-manual-invoice-once"}else{"create-invoice-once"}).await?;
            let rows=read(format!("match $p isa bill_plan,has bill_key {};(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key}};",q(&plan))).await?;ensure!(rows.len()==1,"Invoice duplicated or missing");let id=rows[0]["id"].as_str().unwrap().to_owned();saved[if manual{"manual_invoice"}else{"created_invoice"}]=json!(id);save(&root.join("prepared-run.json"),&saved)?;id
        }else{saved[if manual{"manual_invoice"}else{"created_invoice"}].as_str().context("Create the checkpointed test invoice first")?.to_owned()};
        let before=read(notices(&target)).await?;
        // Resolve templated input through the production workflow engine.
        for workflow in ["mercury-reconcile","mercury-notify"] {
            let key=format!("{phase}-{}",uuid::Uuid::new_v4());
            let row=ok(&router,&tenant,token,"/automation",json!({"action":"run","workflow":workflow,"key":key,"input":{"invoice_id":target}}),None).await?;
            let id=row["id"].as_str().unwrap();completed(&router,&tenant,token,id).await?;
            let replay=ok(&router,&tenant,token,"/automation",json!({"action":"run","workflow":workflow,"key":key,"input":{"invoice_id":target}}),None).await?;ensure!(replay["id"]==id,"Replay created another run");
        }
        let observed=read(invoice(&target)).await?;ensure!(observed.len()==1,"Invoice missing");let after=read(notices(&target)).await?;
        let status=observed[0]["status"].as_str().unwrap();
        save(&root.join(format!("{phase}-observation.json")),&json!({"phase":phase,"invoice":observed[0],"notices":after,"provider_paid_and_application_receipt":status=="paid"&&after.iter().any(|n|n["id"]==format!("{target}-receipt")&&n["status"].as_str().unwrap().starts_with("accepted:"))}))?;
        match phase {
            "processing"|"processing-created"=>ensure!(status=="processing"&&before==after&&!after.iter().any(|n|n["id"].as_str().unwrap().ends_with("-receipt")),"Processing must not produce reminders or receipts"),
            "create"|"create-manual"=>ensure!(status=="open"&&after.iter().filter(|n|n["id"]==format!("{target}-reminder-0")&&n["status"].as_str().unwrap().starts_with("accepted:")).count()==1,"Initial reminder missing or duplicated"),
            _=>ensure!(status=="paid"&&after.iter().filter(|n|n["id"]==format!("{target}-receipt")&&n["status"].as_str().unwrap().starts_with("accepted:")).count()==1,"Mercury must report Paid before a receipt; observed {status}; checkpoint retained"),
        }
        save(&root.join(format!("{phase}-report.json")),&json!({"phase":phase,"passed":true,"invoice":observed[0],"notices":after,"tenant_connections":"Mercury + independent Resend","execution":"native data -> authenticated automation API -> durable worker -> financial/notification plugins","provider_paid_and_application_receipt":phase=="paid"||phase=="paid-manual"}))?;
        println!("Mercury managed {phase} checks passed; provider status {status}; checkpoint retained");Ok::<_,anyhow::Error>(())
    }.await;
    worker.shutdown().await?;
    let folder = root
        .join("checkpoint")
        .join(format!("{phase}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
    }
    driver
        .databases()
        .get(&name)
        .await?
        .export_to_file(
            folder.join(format!("{name}.tql")),
            folder.join(format!("{name}.typedb")),
        )
        .await?;
    save(&folder.join("run.json"), &saved)?;
    outcome
}
