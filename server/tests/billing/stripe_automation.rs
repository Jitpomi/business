//! Real Stripe TEST payments through tenant-owned encrypted connections and worker.
//! Fixture setup attaches Stripe test methods; invoices/collection/notices use plugins.
use super::sandbox_support::{completed, http, ok, save};
use super::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[tokio::test]
#[ignore = "Approved Stripe TEST key, development TypeDB and dev mailbox; creates two preserved synthetic tenants/invoices and reminders"]
async fn stripe_managed_create_invoices_and_reminders() -> Result<()> {
    run("create").await
}
#[tokio::test]
#[ignore = "Collects the checkpointed USD 1 Stripe TEST invoices through tenant-managed plugins; verifies success/decline and receipt"]
async fn stripe_managed_pay_and_receipt() -> Result<()> {
    run("pay").await
}
#[tokio::test]
#[ignore = "Preserved Stripe sandbox checkpoint; verifies restart/replay causes no extra charge/invoice/mail"]
async fn stripe_managed_restart_replay() -> Result<()> {
    run("replay").await
}

fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}
async fn read(state: &TypeDBState, query: &str) -> Result<Vec<Value>> {
    let tx = state
        .driver
        .transaction(&state.database, TransactionType::Read)
        .await?;
    database::documents(&tx, query).await
}
async fn native(app: &axum::Router, tenant: &str, token: &str, query: String) -> Result<()> {
    let v = ok(
        app,
        tenant,
        token,
        "/data",
        json!({"query":query}),
        Some("write"),
    )
    .await?;
    ensure!(
        v["ok"]["answers"].as_array().is_some_and(|a| a.len() == 1),
        "Native write did not commit: {v}"
    );
    Ok(())
}
async fn invoke(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    workflow: &str,
    key: &str,
    invoice: Option<&str>,
) -> Result<Value> {
    let data = json!({"action":"run","workflow":workflow,"key":key,"input":invoice.map(|id|json!({"invoice_id":id})).unwrap_or(Value::Null)});
    let a = ok(app, tenant, token, "/automation", data.clone(), None).await?;
    let id = a["id"].as_str().context("Missing run ID")?;
    let done = completed(app, tenant, token, id).await?;
    let b = ok(app, tenant, token, "/automation", data, None).await?;
    ensure!(b["id"] == id, "Replay duplicated run");
    Ok(done)
}
fn private_dir(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if !p.exists() {
        std::fs::create_dir(p)?;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
    }
    ensure!(
        p.is_absolute() && p.metadata()?.permissions().mode() & 0o077 == 0,
        "Private absolute test directory required"
    );
    Ok(())
}
async fn run(phase: &str) -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development operator only"
    );
    let root = PathBuf::from(std::env::var("STRIPE_TEST_ROOT")?);
    private_dir(&root)?;
    for dir in ["auth", "secrets", "checkpoint"] {
        private_dir(&root.join(dir))?;
    }
    let secrets: HashMap<String, String> =
        dotenvy::from_path_iter(std::env::var("BILLING_TEST_ENV")?)?
            .collect::<std::result::Result<_, _>>()?;
    let stripe = secrets
        .get("STRIPE_SECRET_KEY")
        .context("Missing Stripe test key")?;
    ensure!(
        stripe.starts_with("sk_test_") && secrets["BILLING_TEST_EMAIL"] == "dev@jitpomi.com",
        "TEST key and approved mailbox required"
    );
    if !root.join("vault-key").exists() {
        use aes_gcm::aead::rand_core::RngCore;
        use base64::Engine;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut key = [0u8; 32];
        aes_gcm::aead::OsRng.fill_bytes(&mut key);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("vault-key"))?
            .write_all(
                base64::engine::general_purpose::STANDARD
                    .encode(key)
                    .as_bytes(),
            )?;
    }
    for (key, value) in [
        ("AUTOMATION_ENABLED", "true"),
        ("BILLING_ENABLED", "true"),
        ("AUTH_LOCAL_ENABLED", "true"),
        ("AUTH_OIDC_PROVIDERS_JSON", "[]"),
        ("AUTH_SERVICE_REQUIREMENTS_JSON", "{}"),
        ("STRIPE_CONNECTIONS_JSON", "[]"),
        ("MERCURY_CONNECTIONS_JSON", "[]"),
        ("TENANT_FINANCIAL_SETTINGS_JSON", "[]"),
        ("TENANT_NOTIFICATION_PLUGINS_JSON", "[]"),
    ] {
        std::env::set_var(key, value);
    }
    for key in ["BUSINESS_GATEWAY_SECRET", "AUTOMATION_SECRET_STORE_URL"] {
        std::env::remove_var(key);
    }
    std::env::set_var("AUTH_LOCAL_STORE_DIR", root.join("auth"));
    std::env::set_var("AUTOMATION_SECRET_STORE_DIR", root.join("secrets"));
    std::env::set_var(
        "AUTOMATION_MASTER_KEY_B64",
        std::fs::read_to_string(root.join("vault-key"))?,
    );
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let path = root.join("run.json");
    let mut saved: Value = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path)?)?
    } else {
        ensure!(phase == "create", "Create isolated fixtures first");
        let name = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
        let mut cases = Vec::new();
        for (label, method) in [
            ("success", "pm_card_visa"),
            ("decline", "pm_card_chargeCustomerFail"),
        ] {
            cases.push(json!({"label":label,"test_method":method,"tenant":uuid::Uuid::new_v4().to_string(),"buyer":uuid::Uuid::new_v4().to_string(),"owner":uuid::Uuid::new_v4().to_string(),"customer":uuid::Uuid::new_v4().to_string(),"plan":uuid::Uuid::new_v4().to_string()}));
        }
        let v = json!({"database":name,"cases":cases});
        save(&path, &v)?;
        database::install(&driver, &name).await?;
        v
    };
    let name = saved["database"]
        .as_str()
        .context("Missing database")?
        .to_owned();
    uuid::Uuid::parse_str(
        name.strip_prefix("business_backend_test_")
            .context("Generated database only")?,
    )?;
    database::verify_runtime(&driver, &name).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: name.clone(),
    });
    let (app, transport) = server::app::build_with_plugins(
        state.clone(),
        "stripe-integration-test-signing-key-at-least-32-bytes".into(),
        Arc::new(server::finance::TenantRegistry::default()),
    )?;
    let template: Arc<server::automation::engine::Engine> =
        app.get("automation.engine").context("Automation missing")?;
    let registry: Arc<server::finance::TenantRegistry> = app
        .get("tenant.plugins")
        .context("Plugin resolver missing")?;
    let router = server::app::router(transport);
    // Only fixture creation/attachment uses the raw provider transport. Every invoice
    // and collection operation below resolves the authenticated tenant's connections.
    let fixture = server::plugins::builtin_transport::Providers::connected(
        false,
        Some(stripe.clone()),
        None,
        None,
        None,
    )?;
    let mut tokens = Vec::new();
    for case in saved["cases"].as_array_mut().context("Missing cases")? {
        let tenant = case["tenant"].as_str().unwrap();
        let owner = case["owner"].as_str().unwrap();
        let buyer = case["buyer"].as_str().unwrap();
        let customer = case["customer"].as_str().unwrap();
        let label = case["label"].as_str().unwrap();
        if read(
            &state,
            &format!(
                "match $t isa tenant,has tenant_id {};fetch {{'id':$t.tenant_id}};",
                q(tenant)
            ),
        )
        .await?
        .is_empty()
        {
            let tx = driver.transaction(&name, TransactionType::Write).await?;
            let link = server::auth::local_link("$p", owner)?;
            tx.query(format!("insert $t isa tenant,has tenant_id {},has name 'TEST Stripe {label} merchant',has status 'active',has revision 'initial';$b isa tenant,has tenant_id {},has name 'TEST Stripe buyer',has status 'active',has revision 'initial';$p isa person,has identity_id {},has email {},has name 'Synthetic Stripe owner',has status 'active';{link}(tenant:$t,member:$p) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;(tenant:$b,member:$p) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;$c isa bill_customer,has bill_key {},has name 'TEST Stripe {label}',has bill_email 'dev@jitpomi.com',has bill_policy 'customer',has bill_choice 'manual',has bill_provider 'my-finance',has bill_stripe_id '',has bill_mercury_id '',has bill_bindings '{{}}',has bill_method_plugin '',has bill_payment_method '',has bill_consent '',has bill_revision 'initial';(merchant:$t,buyer:$b,customer:$c) isa bill_account;",q(tenant),q(buyer),q(owner),q(&format!("stripe-{label}@example.invalid")),q(customer))).await?;
            tx.commit().await?;
        }
        let local = server::auth::plugins::local::Local::configured(&name)?;
        if local.lookup(owner, false)?.is_none() {
            local.import(
                owner,
                &format!("stripe-{label}@example.invalid"),
                &bcrypt::hash("stripe-acceptance-password", 4)?,
                "initial",
            )?;
        }
        let login=ok(&router,tenant,"","/authentication",json!({"strategy":"local","email":format!("stripe-{label}@example.invalid"),"password":"stripe-acceptance-password"}),None).await?;
        let token = login["accessToken"]
            .as_str()
            .context("Missing auth proof")?
            .to_owned();
        tokens.push(token.clone());
        if phase == "replay" {
            let query=format!("match $c isa bill_customer,has bill_key {};update $c has bill_consent 'initial',has bill_choice 'automatic';fetch {{'id':$c.bill_key}};",q(customer));
            let (status, body) = http(
                &router,
                buyer,
                &token,
                "/data",
                json!({"query":query}),
                Some("write"),
            )
            .await?;
            ensure!(
                status == 400
                    && body["message"].as_str().is_some_and(
                        |s| s.contains("Combined updates need a combined proposal policy")
                    ),
                "Unsupported consent operation was not rejected clearly"
            );
            save(
                &root.join(format!("onboarding-{label}-observation.json")),
                &json!({"http":status,"response":body,"onboarding_verified":false}),
            )?;
        }
        let store = server::automation::store::Store::new(state.clone(), tenant)?;
        // Tenant B must not inherit A's connections even with identical aliases.
        if store.get("connection", "my-finance").await?.is_none() {
            ensure!(
                registry
                    .resolve(tenant)
                    .await?
                    .financial("my-finance")
                    .is_err(),
                "Connection leaked between tenants"
            );
        }
        for (id, connector, config, credential) in [
            (
                "my-finance",
                "stripe",
                json!({"sandbox":true}),
                json!({"token":stripe}),
            ),
            (
                "my-mail",
                "resend",
                json!({"sandbox":true,"from":secrets["BILLING_EMAIL_FROM"]}),
                json!({"token":secrets["RESEND_API_KEY"]}),
            ),
            ("billing", "billing", json!({}), Value::Null),
        ] {
            if store.get("connection", id).await?.is_none() {
                ok(&router,tenant,&token,"/automation",json!({"action":"connect","id":id,"connector":connector,"config":config,"secrets":credential}),None).await?;
            }
            ok(
                &router,
                tenant,
                &token,
                "/automation",
                json!({"action":"verify","id":id}),
                None,
            )
            .await?;
        }
        ok(&router,tenant,&token,"/automation",json!({"action":"configure_billing","settings":{"sandbox":true,"default_financial":"my-finance","notification":"my-mail","reminder_days":7,"allowed_test_recipients":["dev@jitpomi.com"]}}),None).await?;
        for (id, action) in [
            ("schedule", "enqueue_due"),
            ("issue", "issue"),
            ("collect", "collect_due"),
            ("notify", "notify"),
            ("reconcile", "reconcile"),
        ] {
            if store.get("workflow", id).await?.is_none() {
                ok(&router,tenant,&token,"/automation",json!({"action":"publish","workflow":{"id":id,"revision":"","name":"Stripe sandbox acceptance","enabled":true,"steps":[{"kind":"action","id":"execute","connection":"billing","action":action,"input":if action=="enqueue_due"{Value::Null}else{json!({"invoice_id":{"$ref":"input/invoice_id"}})}}]}}),None).await?;
            }
        }
        if phase == "create" {
            let plan = case["plan"].as_str().unwrap();
            if read(
                &state,
                &format!(
                    "match $p isa bill_plan,has bill_key {};fetch {{'id':$p.bill_key}};",
                    q(plan)
                ),
            )
            .await?
            .is_empty()
            {
                let anchor = chrono::Utc::now().timestamp() - 60;
                native(&router,tenant,&token,format!("match $c isa bill_customer,has bill_key {};insert $p isa bill_plan,has bill_key {},has name 'TEST ONLY Stripe {label} tenant plugin',has bill_amount 100,has bill_due_days 0,has bill_anchor {anchor},has bill_next {anchor},has bill_sequence 0,has bill_interval 'once',has bill_status 'active';(customer:$c,plan:$p) isa bill_plan_owner;fetch {{'id':$p.bill_key}};",q(customer),q(plan))).await?;
            }
            let e = server::billing::engine::Engine::new(
                state.clone(),
                tenant,
                registry.resolve(tenant).await?,
            )?;
            let external = e
                .ensure_customer(&e.customer(customer).await?, "my-finance")
                .await?;
            let method = fixture
                .stripe(
                    reqwest::Method::POST,
                    &format!(
                        "/payment_methods/{}/attach",
                        case["test_method"].as_str().unwrap()
                    ),
                    &format!("fixture-method-{customer}"),
                    &pairs(&[("customer", &external)]),
                )
                .await?;
            let pm = method["id"].as_str().context("Missing test method")?;
            let verified = e
                .runtime
                .financial("my-finance")?
                .method(&external, pm)
                .await?;
            e.attest_method(customer, "my-finance", &verified).await?;
            // Explicit pre-consented synthetic fixture. This is not a public buyer
            // onboarding test: the current compiler rejects combined consent updates.
            // Never seed or change invoice/payment status to make acceptance pass.
            let tx = driver.transaction(&name, TransactionType::Write).await?;
            tx.query(format!("match $c isa bill_customer,has bill_key {};update $c has bill_choice 'automatic',has bill_method_plugin 'my-finance',has bill_payment_method {},has bill_consent 'initial';",q(customer),q(pm))).await?;
            tx.commit().await?;
            case["provider_customer"] = json!(external);
            case["payment_method"] = json!(pm);
        }
    }
    save(&path, &saved)?;
    let tenants: Vec<String> = saved["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["tenant"].as_str().unwrap().to_owned())
        .collect();
    let worker = server::automation::runtime::spawn_all((*template).clone());
    let outcome=async {
        for (n,case) in saved["cases"].as_array_mut().unwrap().iter_mut().enumerate(){
            let tenant=case["tenant"].as_str().unwrap().to_owned();let token=&tokens[n];let label=case["label"].as_str().unwrap().to_owned();let plan=case["plan"].as_str().unwrap().to_owned();
            if phase=="create" {
                invoke(&router,&tenant,token,"schedule","create-invoice-once",None).await?;
                let rows=read(&state,&format!("match $p isa bill_plan,has bill_key {};(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key}};",q(&plan))).await?;ensure!(rows.len()==1,"Missing/duplicate invoice");
                case["invoice"]=rows[0]["id"].clone();
                invoke(&router,&tenant,token,"issue","issue-invoice-once",case["invoice"].as_str()).await?;
            }
            let invoice=case["invoice"].as_str().context("Create checkpoint first")?.to_owned();
            let notice_query=format!("match $i isa bill_invoice,has bill_key {};(invoice:$i,notice:$n) isa bill_notice_owner;$n has bill_key $key;select $n,$key;sort $key;fetch {{'id':$n.bill_key,'status':$n.bill_status}};",q(&invoice));
            let before=read(&state,&notice_query).await?;
            if phase=="pay"||phase=="replay"{invoke(&router,&tenant,token,"collect",&format!("collect-{phase}"),Some(&invoice)).await?;}
            invoke(&router,&tenant,token,"reconcile",&format!("reconcile-{phase}"),Some(&invoice)).await?;
            invoke(&router,&tenant,token,"notify",&format!("notify-{phase}"),Some(&invoice)).await?;
            let rows=read(&state,&format!("match $i isa bill_invoice,has bill_key {};fetch {{'id':$i.bill_key,'external':$i.bill_external_id,'status':$i.bill_status,'plugin':$i.bill_provider,'url':$i.bill_url,'revision':$i.bill_revision}};",q(&invoice))).await?;ensure!(rows.len()==1,"Missing invoice");
            let external=rows[0]["external"].as_str().unwrap();let raw=fixture.observe(Provider::Stripe,external).await?;
            let payments=fixture.stripe(reqwest::Method::GET,"/invoice_payments","",&pairs(&[("invoice",external),("limit","100")])).await?;
            let mut intents=Vec::new();
            for payment in payments["data"].as_array().context("Missing invoice payment records")? {
                if let Some(id)=payment["payment"]["payment_intent"].as_str(){
                    intents.push(fixture.stripe(reqwest::Method::GET,&format!("/payment_intents/{id}"),"",&pairs(&[("expand[]","latest_charge")])).await?);
                }
            }
            let after=read(&state,&notice_query).await?;
            let proof=json!({"invoice":rows[0],"provider":raw,"provider_payments":payments,"payment_intents":intents,"notices":after});save(&root.join(format!("{phase}-{label}-observation.json")),&proof)?;
            ensure!(raw["livemode"]==false&&raw["total"]==100&&raw["currency"]=="usd"&&raw["customer"]==case["provider_customer"]&&raw["metadata"]["business_invoice"]==invoice,"Provider binding/amount/mode mismatch");
            ensure!(rows[0]["plugin"]=="my-finance","Tenant-selected alias ignored");
            let paid=phase!="create"&&label=="success";
            ensure!(rows[0]["status"]==if paid{"paid"}else{"open"},"Unexpected invoice state");
            let count=|suffix:&str|after.iter().filter(|v|v["id"]==format!("{invoice}-{suffix}")&&v["status"].as_str().is_some_and(|s|s.starts_with("accepted:"))).count();
            ensure!(count("reminder-0")==1&&count("receipt")==usize::from(paid)&&after.len()==if paid{2}else{1},"Duplicate/missing/premature notice");
            ensure!(raw["attempt_count"]==if phase=="create"{0}else{1},"Unexpected payment attempt count");
            if paid {ensure!(raw["amount_paid"]==100&&raw["amount_remaining"]==0&&payments["data"].as_array().is_some_and(|a|a.iter().filter(|p|p["status"]=="paid"&&p["amount_paid"]==100).count()==1),"Successful paid record missing");}
            if phase!="create" {
                ensure!(intents.len()==1,"Expected one invoice payment intent");
                let intent=&intents[0];
                ensure!(intent["livemode"]==false&&intent["customer"]==case["provider_customer"]&&intent["amount"]==100,"Payment intent binding mismatch");
                if paid {ensure!(intent["status"]=="succeeded"&&intent["amount_received"]==100&&intent["latest_charge"]["paid"]==true&&intent["latest_charge"]["receipt_url"].as_str().is_some_and(|s|s.starts_with("https://pay.stripe.com/receipts/")),"Charge receipt or successful intent missing");}
                else {ensure!(intent["status"]=="requires_payment_method"&&intent["last_payment_error"]["code"]=="card_declined","Expected actual card decline");}
            }
            let all=read(&state,&format!("match $p isa bill_plan,has bill_key {};(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key}};",q(&plan))).await?;
            ensure!(all.len()==1&&all[0]["id"]==invoice,"Restart duplicated invoice");
            if phase=="replay"{ensure!(before==after,"Restart repeated a notice");}
            // Authenticated A cannot read/write B even though both use the same alias.
            let foreign=&tokens[1-n];
            ensure!(http(&router,&tenant,foreign,"/automation",json!({"action":"list","kind":"connection"}),None).await?.0==401,"Foreign token accepted");
            let query=format!("match $i isa bill_invoice,has bill_key {};fetch {{'id':$i.bill_key}};",q(&invoice));
            let (status,body)=http(&router,&tenants[1-n],foreign,"/data",json!({"query":query}),Some("read")).await?;
            ensure!(status==200&&body["ok"]["answers"].as_array().is_some_and(Vec::is_empty),"TypeDB leaked foreign invoice: {body}");
            let query=format!("match $c isa bill_customer,has bill_key {};update $c has bill_revision 'foreign-attack';fetch {{'id':$c.bill_key}};",q(case["customer"].as_str().unwrap()));
            let (status,body)=http(&router,&tenants[1-n],foreign,"/data",json!({"query":query}),Some("write")).await?;
            ensure!(status>=400||body["ok"]["answers"].as_array().is_some_and(Vec::is_empty),"TypeDB allowed foreign mutation: {body}");
            let customer=read(&state,&format!("match $c isa bill_customer,has bill_key {};fetch {{'revision':$c.bill_revision}};",q(case["customer"].as_str().unwrap()))).await?;
            ensure!(customer.len()==1&&customer[0]["revision"]=="initial","Foreign update changed customer consent revision");
        }
        Ok::<_,anyhow::Error>(())
    }.await;
    worker.abort();
    let _ = worker.await;
    save(&path, &saved)?;
    let folder = root
        .join("checkpoint")
        .join(format!("{phase}-{}", uuid::Uuid::new_v4().simple()));
    private_dir(&folder)?;
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
    outcome?;
    let graph = serde_json::to_string(&database::snapshot::graph(&driver, &name, &[]).await?)?;
    ensure!(
        !graph.contains(stripe) && !graph.contains(&secrets["RESEND_API_KEY"]),
        "Credential leaked into TypeDB"
    );
    save(
        &root.join(format!("{phase}-report.json")),
        &json!({"passed":true,"phase":phase,"execution":"native data -> authenticated automation -> durable worker -> tenant-owned financial and notification connections","same_test_provider_account":true,"live_data_untouched":true,"checkpoint_retained":true}),
    )?;
    println!("Stripe managed {phase} checks passed; checkpoint retained");
    Ok(())
}
