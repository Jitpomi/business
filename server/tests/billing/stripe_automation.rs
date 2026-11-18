//! Real Stripe TEST payments through tenant-owned encrypted connections and worker.
//! Fixture setup attaches Stripe test methods; invoices/collection/notices use plugins.
use super::sandbox_support::{completed, http, ok, save};
use super::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[tokio::test]
#[ignore = "Approved Stripe TEST key, development TypeDB and dev mailbox; creates three preserved synthetic tenants/invoices and reminders"]
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
#[tokio::test]
#[ignore = "Approved checkpoint; native consent security cases only, no payments or mail"]
async fn stripe_managed_consent_security() -> Result<()> {
    run("security").await
}

#[tokio::test]
#[ignore = "Preserved checkpoint; public buyer/seller consent status, no payment or mail"]
async fn stripe_managed_consent_status() -> Result<()> {
    run("status").await
}

#[tokio::test]
#[ignore = "Preserved new Stripe TEST checkout checkpoint; submits one partial refund through tenant plugins and dog-queue"]
async fn stripe_managed_checkout_refund() -> Result<()> {
    run("refund").await
}

#[tokio::test]
#[ignore = "Preserved Stripe TEST checkpoint; verifies trusted saved-method checkout return context, no payment"]
async fn stripe_managed_checkout_setup_return() -> Result<()> {
    run("setup-return").await
}

fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}
pub(super) async fn read(state: &TypeDBState, query: &str) -> Result<Vec<Value>> {
    let tx = state
        .driver
        .transaction(&state.database, TransactionType::Read)
        .await?;
    database::documents(&tx, query).await
}
pub(super) async fn native(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    query: String,
) -> Result<()> {
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
fn grant(
    customer: &str,
    actor: &str,
    id: &str,
    terms: (&str, &str),
    plugin: &str,
    method: &str,
    time: &str,
) -> String {
    let (revision, plan) = terms;
    let at = chrono::Utc::now().timestamp();
    format!("match $c isa bill_customer,has bill_key {};$by isa person,has identity_id {};let $now=request_now();$approved isa bill_plan,has bill_key {};insert $e isa bill_consent_evidence,has bill_key {},has bill_revision {},has consent_plan_count 1,has bill_updated {at},has consent_claim {},has method_plugin {},has method_ref {},has consent_at == {time};(customer:$c,actor:$by,evidence:$e) isa bill_consent_owner;(evidence:$e,plan:$approved) isa bill_consent_plan;fetch {{'id':$e.bill_key,'at':$e.consent_at}};",q(customer),q(actor),q(plan),q(id),q(revision),q(&format!("{customer}:{revision}:{id}")),q(plugin),q(method))
}
pub(super) fn grant_template(
    customer: &str,
    id: &str,
    revision: &str,
    plan: &str,
    plugin: &str,
    method: &str,
) -> String {
    let mut query = include_str!("../../public/queries/data/billing-grant-consent.tql").to_owned();
    for (key, value) in [
        ("customer_id", customer),
        ("request_id", id),
        ("expected_revision", revision),
        ("plan_id", plan),
        ("plugin_id", plugin),
        ("method_id", method),
        ("consent_claim", &format!("{customer}:{revision}:{id}")),
    ] {
        query = query.replace(&format!("@@{key}@@"), &q(value));
    }
    query
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
        crate::test_driver::connect(
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
            ("revoked", "pm_card_visa"),
        ] {
            cases.push(json!({"label":label,"test_method":method,"tenant":uuid::Uuid::new_v4().to_string(),"buyer":uuid::Uuid::new_v4().to_string(),"owner":uuid::Uuid::new_v4().to_string(),"buyer_owner":uuid::Uuid::new_v4().to_string(),"consent_key":uuid::Uuid::new_v4().to_string(),"revoked_revision":uuid::Uuid::new_v4().to_string(),"renewed_consent_key":uuid::Uuid::new_v4().to_string(),"future_plan":uuid::Uuid::new_v4().to_string(),"unapproved_plan":uuid::Uuid::new_v4().to_string(),"customer":uuid::Uuid::new_v4().to_string(),"plan":uuid::Uuid::new_v4().to_string()}));
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
    ensure!(
        driver
            .databases()
            .get(&name)
            .await?
            .schema()
            .await?
            .contains("fun bill_has_consent"),
        "Use a new acceptance database; preserve old checkpoints"
    );
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
    let router = server::app::runtime_router(&app, transport).await?;
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
    let mut buyer_tokens = Vec::new();
    for case in saved["cases"].as_array_mut().context("Missing cases")? {
        let tenant = case["tenant"].as_str().unwrap();
        let owner = case["owner"].as_str().unwrap();
        let buyer = case["buyer"].as_str().unwrap();
        let buyer_owner = case["buyer_owner"]
            .as_str()
            .context("Use a new native-consent acceptance root; old fixtures are preserved")?;
        let customer = case["customer"].as_str().unwrap();
        let label = case["label"].as_str().unwrap();
        eprintln!("Preparing native buyer consent for {label}");
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
        if read(
            &state,
            &format!(
                "match $by isa person,has identity_id {};fetch {{'id':$by.identity_id}};",
                q(buyer_owner)
            ),
        )
        .await?
        .is_empty()
        {
            let tx = driver.transaction(&name, TransactionType::Write).await?;
            let buyer_link = server::auth::local_link("$by", buyer_owner)?;
            tx.query(format!("match $b isa tenant,has tenant_id {};$p isa person,has identity_id {};$m isa membership,links(tenant:$b,member:$p);delete $m;",q(buyer),q(owner))).await?;
            tx.query(format!("match $b isa tenant,has tenant_id {};insert $by isa person,has identity_id {},has email {},has name 'Synthetic Stripe buyer',has status 'active';{buyer_link}(tenant:$b,member:$by) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;",q(buyer),q(buyer_owner),q(&format!("stripe-{label}-buyer@example.invalid")))).await?;
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
        if local.lookup(buyer_owner, false)?.is_none() {
            local.import(
                buyer_owner,
                &format!("stripe-{label}-buyer@example.invalid"),
                &bcrypt::hash("stripe-acceptance-password", 4)?,
                "initial",
            )?;
        }
        let login=ok(&router,buyer,"","/authentication",json!({"strategy":"local","email":format!("stripe-{label}-buyer@example.invalid"),"password":"stripe-acceptance-password"}),None).await?;
        let buyer_token = login["accessToken"]
            .as_str()
            .context("Missing buyer proof")?
            .to_owned();
        buyer_tokens.push(buyer_token.clone());
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
            if label == "revoked" {
                let other = case["unapproved_plan"].as_str().unwrap();
                if read(
                    &state,
                    &format!(
                        "match $p isa bill_plan,has bill_key {};fetch {{'id':$p.bill_key}};",
                        q(other)
                    ),
                )
                .await?
                .is_empty()
                {
                    let at = chrono::Utc::now().timestamp() - 60;
                    native(&router,tenant,&token,format!("match $c isa bill_customer,has bill_key {};insert $p isa bill_plan,has bill_key {},has name 'TEST unapproved scope control',has bill_amount 100,has bill_due_days 0,has bill_anchor {at},has bill_next {at},has bill_sequence 0,has bill_interval 'once',has bill_status 'active';(customer:$c,plan:$p) isa bill_plan_owner;fetch {{'id':$p.bill_key}};",q(customer),q(other))).await?;
                }
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
            let methods = ok(
                &router,
                buyer,
                &buyer_token,
                "/financial",
                json!({"action":"payment_methods","customer_id":customer,"plugin_id":"my-finance"}),
                None,
            )
            .await?;
            ensure!(
                methods["methods"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|m| m["id"] == pm)),
                "Public financial plugin did not verify saved test method"
            );
            let consent_key = case["consent_key"].as_str().unwrap();
            if read(&state,&format!("match $e isa bill_consent_evidence,has bill_key {};fetch {{'id':$e.bill_key}};",q(consent_key))).await?.is_empty(){
                native(&router,buyer,&buyer_token,format!("match $c isa bill_customer,has bill_key {};update $c has bill_method_plugin 'my-finance';fetch {{'id':$c.bill_key}};",q(customer))).await?;
                native(&router,buyer,&buyer_token,format!("match $c isa bill_customer,has bill_key {};update $c has bill_payment_method {};fetch {{'id':$c.bill_key}};",q(customer),q(pm))).await?;
                if label=="success" {
                    for field in ["bill_consent","bill_choice"] {
                        let proposed=if field=="bill_choice"{"automatic"}else{"initial"};let query=format!("match $c isa bill_customer,has bill_key {};update $c has {field} {};fetch {{'id':$c.bill_key}};",q(customer),q(proposed));
                        let (status,body)=http(&router,buyer,&buyer_token,"/data",json!({"query":query}),Some("write")).await?;
                        ensure!(status==403,"Legacy flag remained writable: {body}");
                    }
                }
                if label=="success" { for (attack,actor,revision,method,time) in [("seller",owner,"initial",pm,"$now"),("wrong-revision",buyer_owner,"forged",pm,"$now"),("wrong-method",buyer_owner,"initial","pm_unverified","$now"),("forged-time",buyer_owner,"initial",pm,"2000-01-01T00:00:00")] {
                    let query=grant(customer,actor,&format!("attack-{attack}-{customer}"),(revision,plan),"my-finance",method,time);
                    let (status,body)=http(&router,if attack=="seller"{tenant}else{buyer},if attack=="seller"{&token}else{&buyer_token},"/data",json!({"query":query}),Some("write")).await?;
                    save(&root.join(format!("denied-{label}-{attack}.json")),&json!({"status":status,"body":body}))?;
                    ensure!(status==403||(status==200&&body["ok"]["answers"].as_array().is_some_and(Vec::is_empty)),"Forged grant not denied by native policy: {attack}: {body}");
                    ensure!(read(&state,&format!("match $e isa bill_consent_evidence,has bill_key {};fetch {{'id':$e.bill_key}};",q(&format!("attack-{attack}-{customer}")))).await?.is_empty(),"Failed grant left a record");
                }
                }
                let data=json!({"query":grant_template(customer,consent_key,"initial",plan,"my-finance",pm)});
                let (a,b)=tokio::join!(http(&router,buyer,&buyer_token,"/data",data.clone(),Some("write")),http(&router,buyer,&buyer_token,"/data",data,Some("write")));
                let responses=[a?,b?];
                ensure!(responses.iter().filter(|(status,body)|*status==200&&body["ok"]["answers"].as_array().is_some_and(|a|a.len()==1)).count()==1,"Concurrent grant did not commit exactly once: {responses:?}");
                let fork=json!({"query":grant(customer,buyer_owner,consent_key,("initial",plan),"my-finance",pm,"$now")});
                let (status,body)=http(&router,buyer,&buyer_token,"/data",fork,Some("write")).await?;
                save(&root.join(format!("duplicate-{label}-observation.json")),&json!({"status":status,"body":body}))?;ensure!((400..500).contains(&status)||(status==200&&body["ok"]["answers"].as_array().is_some_and(Vec::is_empty)),"Duplicate grant did not return a clear rejection: {body}");

            }
            let proof=read(&state,&format!("match $c isa bill_customer,has bill_key {};(customer:$c,actor:$by,evidence:$e) isa bill_consent_owner;$e has consent_claim $claim;(evidence:$e,plan:$approved) isa bill_consent_plan;fetch {{'grant':$e.bill_key,'actor':$by.identity_id,'at':$e.consent_at,'revision':$e.bill_revision,'plan':$approved.bill_key,'mode':bill_payment_choice($c),'legacy_choice':$c.bill_choice,'legacy_consent':$c.bill_consent}};",q(customer))).await?;
            ensure!(
                proof.len() == 1
                    && proof[0]["actor"] == buyer_owner
                    && proof[0]["plan"] == plan
                    && proof[0]["mode"] == "automatic"
                    && proof[0]["legacy_choice"] == "manual"
                    && proof[0]["legacy_consent"] == "",
                "Native grant did not become the sole authority: {proof:?}"
            );
            save(
                &root.join(format!("consent-{label}-observation.json")),
                &json!(proof),
            )?;
            eprintln!("Native buyer consent ready for {label}");
            case["provider_customer"] = json!(external);
            case["payment_method"] = json!(pm);
        }
    }
    save(&path, &saved)?;
    if phase == "refund" && saved["checkout_refund"].is_null() {
        saved["checkout_refund"] = json!(uuid::Uuid::new_v4().to_string());
        save(&path, &saved)?;
    }
    let tenants: Vec<String> = saved["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["tenant"].as_str().unwrap().to_owned())
        .collect();
    let worker = server::background::spawn_all((*template).clone());
    let outcome=async {
        if phase=="setup-return" {
            let case=&saved["cases"][0];let buyer=case["buyer"].as_str().unwrap();
            let result=ok(&router,buyer,&buyer_tokens[0],"/financial",json!({"action":"setup_method","customer_id":case["customer"],"plugin_id":"my-finance","save_only":true,"checkout":true}),None).await?;
            let link=reqwest::Url::parse(result["url"].as_str().context("Setup URL")?)?;
            ensure!(link.scheme()=="https" && link.host_str()==Some("checkout.stripe.com"));
            let id=link.path_segments().and_then(|mut p|p.next_back()).context("Checkout session id")?;ensure!(id.starts_with("cs_test_"));
            let session=fixture.stripe(reqwest::Method::GET,&format!("/checkout/sessions/{id}"),"",&[]).await?;
            ensure!(session["mode"]=="setup" && session["customer"]==case["provider_customer"] && session["metadata"]["business_customer"]==case["customer"] && session["metadata"]["purpose"]=="save_only");
            for field in ["success_url","cancel_url"] {
                let target=reqwest::Url::parse(session[field].as_str().context("Return URL")?)?;let context=target.query_pairs().collect::<HashMap<_,_>>();
                ensure!(target.origin().ascii_serialization()=="https://jitpomi.com" && target.path()=="/checkout/" && context.get("merchant").map(|v|v.as_ref())==case["tenant"].as_str() && context.get("plugin").is_some_and(|v|v=="my-finance"),"Saved-method return lost trusted merchant/plugin context");
                if field=="success_url" {ensure!(context.get("setup").is_some_and(|v|v=="{CHECKOUT_SESSION_ID}") && context.get("customer").map(|v|v.as_ref())==case["customer"].as_str());}
            }
            save(&root.join("checkout-setup-return.json"),&json!({"mode":session["mode"],"success_url":session["success_url"],"cancel_url":session["cancel_url"],"live":session["livemode"]}))?;return Ok(());
        }
        if phase=="refund" {
            let case=&saved["cases"][0];let tenant=case["tenant"].as_str().unwrap();let owner=case["owner"].as_str().unwrap();let plan=case["plan"].as_str().unwrap();
            let engine=server::billing::engine::Engine::new(state.clone(),tenant,registry.resolve(tenant).await?)?;
            engine.refresh_commerce_plugins().await?;
            let invoice=read(&state,&format!("match $p isa bill_plan,has bill_key {};(plan:$p,invoice:$i) isa bill_invoice_owner;$i has bill_status 'paid';fetch {{'id':$i.bill_key,'external':$i.bill_external_id}};",q(plan))).await?;
            ensure!(invoice.len()==1,"Pay the isolated test invoice first");let local=invoice[0]["id"].as_str().unwrap();let external=invoice[0]["external"].as_str().unwrap();let id=saved["checkout_refund"].as_str().unwrap();
            if read(&state,&format!("match $r isa commerce_refund,has cx_id {};fetch {{'id':$r.cx_id}};",q(id))).await?.is_empty(){
                native(&router,tenant,&tokens[0],format!("match $i isa bill_invoice,has bill_key {};$actor isa person,has identity_id {};let $now=request_now();insert $r isa commerce_refund,has cx_id {},has cx_created == $now,has cx_amount 40,has cx_refund_status 'queued',has cx_busy {};(invoice:$i,request:$r,actor:$actor) isa commerce_refund_owner;fetch {{'id':$r.cx_id}};",q(local),q(owner),q(id),q(&format!("{local}:refund")))).await?;
            }
            ok(&router,tenant,&tokens[0],"/automation",json!({"action":"publish","workflow":{"id":"checkout-refund","revision":"","name":"TEST checkout refund","enabled":true,"steps":[{"kind":"action","id":"execute","connection":"billing","action":"tick","input":Value::Null}]}}),None).await?;
            invoke(&router,tenant,&tokens[0],"checkout-refund","checkout-refund-initial",None).await?;
            invoke(&router,tenant,&tokens[0],"checkout-refund","checkout-refund-replay",None).await?;
            let runtime=registry.resolve(tenant).await?;let observed=runtime.financial("my-finance")?.find_refund(external,id).await?.context("Provider refund not found")?;
            ensure!(observed.state==server::finance::RefundState::Succeeded && observed.amount_minor==40 && observed.request_id==id && observed.invoice_id==external,"Provider refund binding mismatch");
            let rows=read(&state,&format!("match $r isa commerce_refund,has cx_id {};fetch {{'status':$r.cx_refund_status,'amount':$r.cx_amount,'external':$r.cx_external}};",q(id))).await?;
            ensure!(rows.len()==1 && rows[0]["status"]=="succeeded" && rows[0]["amount"]==40 && rows[0]["external"]==observed.external_id);
            let notices=read(&state,&format!("match $n isa bill_notice,has bill_key {};fetch {{'id':$n.bill_key,'status':$n.bill_status}};",q(&format!("{id}-refund-receipt")))).await?;
            ensure!(notices.len()==1 && notices[0]["status"].as_str().is_some_and(|s|s.starts_with("accepted:")),"Refund receipt not accepted exactly once");
            save(&root.join("refund-observation.json"),&json!({"native":rows,"provider":observed,"notices":notices}))?;
            return Ok(());
        }
        if phase=="status" {
            let case=&saved["cases"][0];
            let query=include_str!("../../public/queries/data/billing-customers.tql");
            for (tenant,token) in [(case["buyer"].as_str().unwrap(),buyer_tokens[0].as_str()),(case["tenant"].as_str().unwrap(),tokens[0].as_str())] {
                let response=ok(&router,tenant,token,"/data",json!({"query":query}),Some("read")).await?;
                let rows=&response["ok"]["answers"];
                let row=rows.as_array().context("Missing status rows")?.iter().map(|r|&r["data"]).find(|r|r["id"]==case["customer"]).context("Customer status missing")?;
                ensure!(row["choice"]=="automatic","Buyer/seller status disagrees with native consent: {row}");
            }
            return Ok(());
        }
        if phase=="security" {super::consent_security::check(&router,&state,&saved["cases"][0],&buyer_tokens[0],&tokens[0],&root).await?;return Ok(());}
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
            if phase=="pay"&&label=="revoked" {
                let buyer=case["buyer"].as_str().unwrap();let buyer_token=&buyer_tokens[n];let customer=case["customer"].as_str().unwrap();let revision=case["revoked_revision"].as_str().unwrap();
                let unapproved=case["unapproved_plan"].as_str().unwrap();
                let other=read(&state,&format!("match $p isa bill_plan,has bill_key {};(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key}};",q(unapproved))).await?;ensure!(other.len()==1,"Unapproved scope-control invoice missing");
                let other_id=other[0]["id"].as_str().unwrap();
                invoke(&router,&tenant,token,"issue","issue-unapproved-plan-once",Some(other_id)).await?;
                invoke(&router,&tenant,token,"collect","collect-unapproved-plan",Some(other_id)).await?;
                let other=read(&state,&format!("match $i isa bill_invoice,has bill_key {};fetch {{'external':$i.bill_external_id,'choice':$i.bill_choice}};",q(other_id))).await?;
                let raw=fixture.observe(Provider::Stripe,other[0]["external"].as_str().unwrap()).await?;
                save(&root.join("unapproved-plan-observation.json"),&json!({"invoice":other_id,"choice":other[0]["choice"],"provider":raw}))?;
                ensure!(other[0]["choice"]=="manual"&&raw["attempt_count"]==0&&raw["status"]=="open","An unapproved plan was automatically collected");
                let current=read(&state,&format!("match $c isa bill_customer,has bill_key {};fetch {{'revision':$c.bill_revision}};",q(customer))).await?;
                if current[0]["revision"]=="initial"{native(&router,buyer,buyer_token,format!("match $c isa bill_customer,has bill_key {};update $c has bill_revision {};fetch {{'id':$c.bill_key}};",q(customer),q(revision))).await?;}
                // A previous grant cannot be revived by restoring its old revision.
                let query=format!("match $c isa bill_customer,has bill_key {};update $c has bill_revision 'initial';fetch {{'id':$c.bill_key}};",q(customer));
                let (status,body)=http(&router,buyer,buyer_token,"/data",json!({"query":query}),Some("write")).await?;ensure!(status>=400||body["ok"]["answers"].as_array().is_some_and(Vec::is_empty),"Revoked revision restored");
                invoke(&router,&tenant,token,"collect","collect-after-revocation",Some(&invoice)).await?;
                let next=case["future_plan"].as_str().unwrap();
                if read(&state,&format!("match $p isa bill_plan,has bill_key {};fetch {{'id':$p.bill_key}};",q(next))).await?.is_empty(){let at=chrono::Utc::now().timestamp()+86400;native(&router,&tenant,token,format!("match $c isa bill_customer,has bill_key {};insert $p isa bill_plan,has bill_key {},has name 'TEST future consent scope',has bill_amount 100,has bill_due_days 0,has bill_anchor {at},has bill_next {at},has bill_sequence 0,has bill_interval 'once',has bill_status 'active';(customer:$c,plan:$p) isa bill_plan_owner;fetch {{'id':$p.bill_key}};",q(customer),q(next))).await?;}
                let _=ok(&router,buyer,buyer_token,"/financial",json!({"action":"payment_methods","customer_id":customer,"plugin_id":"my-finance"}),None).await?;
                let key=case["renewed_consent_key"].as_str().unwrap();
                if read(&state,&format!("match $e isa bill_consent_evidence,has bill_key {};fetch {{'id':$e.bill_key}};",q(key))).await?.is_empty(){native(&router,buyer,buyer_token,grant_template(customer,key,revision,next,"my-finance",case["payment_method"].as_str().unwrap())).await?;}
            }
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
            ensure!(raw["attempt_count"]==if phase=="create"||label=="revoked"{0}else{1},"Unexpected payment attempt count");
            if paid {ensure!(raw["amount_paid"]==100&&raw["amount_remaining"]==0&&payments["data"].as_array().is_some_and(|a|a.iter().filter(|p|p["status"]=="paid"&&p["amount_paid"]==100).count()==1),"Successful paid record missing");}
            if phase!="create"&&label!="revoked" {
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
            let other=(n+1)%tokens.len();
            let foreign=&tokens[other];
            ensure!(http(&router,&tenant,foreign,"/automation",json!({"action":"list","kind":"connection"}),None).await?.0==401,"Foreign token accepted");
            let query=format!("match $i isa bill_invoice,has bill_key {};fetch {{'id':$i.bill_key}};",q(&invoice));
            let (status,body)=http(&router,&tenants[other],foreign,"/data",json!({"query":query}),Some("read")).await?;
            ensure!(status==200&&body["ok"]["answers"].as_array().is_some_and(Vec::is_empty),"TypeDB leaked foreign invoice: {body}");
            let query=format!("match $c isa bill_customer,has bill_key {};update $c has bill_revision 'foreign-attack';fetch {{'id':$c.bill_key}};",q(case["customer"].as_str().unwrap()));
            let (status,body)=http(&router,&tenants[other],foreign,"/data",json!({"query":query}),Some("write")).await?;
            ensure!(status>=400||body["ok"]["answers"].as_array().is_some_and(Vec::is_empty),"TypeDB allowed foreign mutation: {body}");
            let customer=read(&state,&format!("match $c isa bill_customer,has bill_key {};fetch {{'revision':$c.bill_revision}};",q(case["customer"].as_str().unwrap()))).await?;
            ensure!(customer.len()==1&&customer[0]["revision"]==if label=="revoked"&&phase!="create"{case["revoked_revision"].clone()}else{json!("initial")},"Foreign update changed customer consent revision");
        }
        Ok::<_,anyhow::Error>(())
    }.await;
    worker.shutdown().await?;
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
        &json!({"passed":true,"phase":phase,"execution":if phase=="status" {"authenticated public data queries; no provider calls"} else if phase=="security" {"native policy security checks and tenant financial verification; no invoice, payment or mail calls"} else {"native data -> authenticated automation -> durable worker -> tenant-owned financial and notification connections"},"same_test_provider_account":true,"public_buyer_consent":true,"synthetic_preconsent_bypass":false,"live_data_untouched":true,"checkpoint_retained":true}),
    )?;
    println!("Stripe managed {phase} checks passed; checkpoint retained");
    Ok(())
}
