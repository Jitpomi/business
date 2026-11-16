//! Contract proof using a third-party-style bank and mail plugin, without vendor credentials.
use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use server::finance::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Bank {
    invoices: Mutex<HashMap<String, InvoiceObservation>>,
    requests: Mutex<HashMap<String, InvoiceRequest>>,
    attempts: Mutex<HashMap<String, usize>>,
    fail_detach: std::sync::atomic::AtomicBool,
}
#[async_trait]
impl FinancialPlugin for Bank {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_customers: true,
            automatic_collection: true,
            saved_methods: true,
            ..Capabilities::default()
        }
    }
    fn ready(&self) -> Result<()> {
        Ok(())
    }
    async fn create_customer(&self, key: &str, _name: &str, _email: &str) -> Result<String> {
        Ok(format!("bank/customer/{key}"))
    }
    async fn create_invoice(&self, r: &InvoiceRequest) -> Result<String> {
        let external = format!("bank/invoice/{}", r.id);
        self.requests
            .lock()
            .unwrap()
            .insert(external.clone(), r.clone());
        self.invoices
            .lock()
            .unwrap()
            .entry(external.clone())
            .or_insert(InvoiceObservation {
                external_id: external.clone(),
                customer: r.customer.clone(),
                invoice_id: r.id.clone(),
                amount_minor: r.amount_minor,
                currency: r.currency.clone(),
                state: InvoiceState::Open,
                payment_url: format!("https://bank.example.invalid/invoice/{}", r.id),
            });
        Ok(external)
    }
    async fn finalize_invoice(&self, _r: &InvoiceRequest, id: &str) -> Result<InvoiceObservation> {
        self.observe_invoice(id).await
    }
    async fn observe_invoice(&self, id: &str) -> Result<InvoiceObservation> {
        self.invoices
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown invoice"))
    }
    async fn method(&self, customer: &str, id: &str) -> Result<SavedMethod> {
        ensure!(id == "bank/method/one", "Foreign method");
        Ok(SavedMethod {
            id: id.into(),
            customer: customer.into(),
            kind: "bank_account".into(),
            brand: None,
            bank_name: Some("Test bank".into()),
            last4: Some("1234".into()),
            exp_month: None,
            exp_year: None,
        })
    }
    async fn methods(&self, customer: &str) -> Result<Vec<SavedMethod>> {
        Ok(vec![self.method(customer, "bank/method/one").await?])
    }
    async fn setup_method(
        &self,
        _customer: &str,
        _local: &str,
        _bank: bool,
        _return_url: &str,
        _key: &str,
    ) -> Result<String> {
        Ok("https://bank.example.invalid/setup".into())
    }
    async fn confirm_method(&self, _customer: &str, _local: &str, session: &str) -> Result<String> {
        ensure!(session == "setup-owned", "Wrong setup session");
        Ok("bank/method/one".into())
    }
    async fn detach_method(&self, customer: &str, method: &str, _key: &str) -> Result<()> {
        self.method(customer, method).await?;
        ensure!(
            !self
                .fail_detach
                .swap(false, std::sync::atomic::Ordering::SeqCst),
            "Synthetic lost detach acknowledgement"
        );
        Ok(())
    }
    async fn collect(&self, id: &str, _key: &str, _method: &str) -> Result<()> {
        *self.attempts.lock().unwrap().entry(id.into()).or_default() += 1;
        if self.requests.lock().unwrap()[id]
            .description
            .contains("decline")
        {
            anyhow::bail!("Synthetic bank decline");
        }
        self.invoices.lock().unwrap().get_mut(id).unwrap().state = InvoiceState::Paid;
        Ok(())
    }
}
#[derive(Default)]
struct Mail {
    messages: Mutex<HashMap<String, Message>>,
}
#[async_trait]
impl NotificationPlugin for Mail {
    async fn send(&self, m: &Message) -> Result<String> {
        self.messages
            .lock()
            .unwrap()
            .entry(m.key.clone())
            .or_insert(m.clone());
        Ok(format!("test-delivery-{}", m.key))
    }
}
struct BankConnector {
    tenants: HashMap<String, (Arc<Bank>, Arc<Mail>)>,
}
#[async_trait]
impl server::automation::connectors::Connector for BankConnector {
    fn name(&self) -> &str {
        "community-bank"
    }
    fn actions(&self) -> Vec<server::automation::model::ActionSpec> {
        vec![]
    }
    fn validate(&self, c: &serde_json::Value, s: &serde_json::Value) -> Result<()> {
        ensure!(c == &serde_json::json!({"sandbox":true}) && s.is_null());
        Ok(())
    }
    async fn verify(&self, c: &serde_json::Value, s: &serde_json::Value) -> Result<()> {
        self.validate(c, s)
    }
    fn financial(
        &self,
        c: &server::automation::connectors::Invocation,
    ) -> Result<Option<Arc<dyn FinancialPlugin>>> {
        Ok(Some(
            self.tenants
                .get(&c.tenant)
                .ok_or_else(|| anyhow::anyhow!("Unknown tenant"))?
                .0
                .clone(),
        ))
    }
    fn notification(
        &self,
        c: &server::automation::connectors::Invocation,
    ) -> Result<Option<Arc<dyn NotificationPlugin>>> {
        Ok(Some(
            self.tenants
                .get(&c.tenant)
                .ok_or_else(|| anyhow::anyhow!("Unknown tenant"))?
                .1
                .clone(),
        ))
    }
    async fn execute(
        &self,
        _: &server::automation::connectors::Invocation,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value> {
        anyhow::bail!("Use the billing capability")
    }
}
#[tokio::test]
async fn registry_is_tenant_scoped_and_open_to_another_bank() -> Result<()> {
    let mut a = TenantRuntime::new("tenant-a".into(), true);
    a.register("community-bank", Arc::new(Bank::default()))?;
    ensure!(
        a.register("community-bank", Arc::new(Bank::default()))
            .is_err(),
        "Duplicate registration accepted"
    );
    let b = TenantRuntime::new("tenant-b".into(), true);
    let mut registry = TenantRegistry::default();
    registry.register(a)?;
    registry.register(b)?;
    ensure!(
        registry
            .get("tenant-a")?
            .financial("community-bank")?
            .capabilities()
            .automatic_collection,
        "Custom capability missing"
    );
    ensure!(
        registry
            .get("tenant-b")?
            .financial("community-bank")
            .is_err(),
        "Tenant plugin leaked"
    );
    ensure!(registry.get("unknown").is_err(), "Unknown tenant fell back");
    let p = registry.get("tenant-a")?.financial("community-bank")?;
    ensure!(
        p.history("invoice").await.is_err(),
        "Unsupported capability fabricated a success"
    );
    Ok(())
}
#[tokio::test]
async fn notification_plugin_is_independent_of_financial_connections() -> Result<()> {
    let mail = Arc::new(Mail::default());
    let mut runtime = TenantRuntime::new("tenant".into(), true);
    runtime
        .allowed_test_recipients
        .push("test@example.invalid".into());
    runtime.notification = Some(mail.clone());
    runtime
        .send(Message {
            to: "test@example.invalid".into(),
            subject: "Test".into(),
            text: "Body".into(),
            key: "one".into(),
        })
        .await?;
    ensure!(
        mail.messages.lock().unwrap().len() == 1,
        "Mail did not dispatch independently"
    );
    ensure!(
        runtime
            .send(Message {
                to: "other@example.invalid".into(),
                subject: "Test".into(),
                text: "Body".into(),
                key: "two".into()
            })
            .await
            .is_err(),
        "Sandbox recipient escaped policy"
    );
    Ok(())
}
#[tokio::test]
#[ignore = "Development TypeDB, generated disposable database; third bank/mail plugins, no external provider or email calls"]
async fn two_tenants_complete_billing_using_independent_plugins() -> Result<()> {
    let _environment = environment::Environment::capture(&["BILLING_ENABLED"]);
    use server::{billing::engine::Engine, database, typedb::TypeDBState};
    use typedb_driver::TransactionType;
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development credentials only"
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
    let name = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Disposable plugin database: {name}");
    database::install(&driver, &name)
        .await
        .context("install plugin fixture schema")?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: name.clone(),
    });
    let result=async{
        eprintln!("Plugin fixture setup");
        let mut fixtures=vec![];
        let mut registry=TenantRegistry::default();
        for _ in 0..2 {
            let tenant=uuid::Uuid::new_v4().to_string();let bank=Arc::new(Bank::default());let mail=Arc::new(Mail::default());
            let mut runtime=TenantRuntime::new(tenant.clone(),true);runtime.register("community-bank",bank.clone())?;runtime.default_financial=Some("community-bank".into());runtime.notification=Some(mail.clone());runtime.allowed_test_recipients.push("test@example.invalid".into());
            registry.register(runtime.clone())?;
            let engine=Engine::new(state.clone(),&tenant,Arc::new(runtime))?;
            let identity=uuid::Uuid::new_v4();let email=format!("{tenant}@example.invalid");let password_hash=bcrypt::hash("plugin-owner-test-password",4)?;
            let tx=driver.transaction(&name,TransactionType::Write).await?;
            let auth_link_0=server::auth::local_link("$u",&identity.to_string())?;server::auth::plugins::local::Local::configured(&name)?.import(&identity.to_string(),&email.to_string(),&password_hash.to_string(),"initial")?;
tx.query(format!(r#"insert $t isa tenant,has tenant_id "{tenant}",has name "Plugin tenant",has status "active",has revision "initial";$u isa person,has identity_id "{identity}",has email "{email}",has name "Owner",has status "active"; {auth_link_0}(tenant:$t,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;"#)).await?;tx.commit().await?;
            for label in ["success","decline"] {
                let customer=uuid::Uuid::new_v4();let buyer=uuid::Uuid::new_v4();let plan=uuid::Uuid::new_v4();let now=chrono::Utc::now().timestamp()-10;let grant=payment_fixture::payment_grant_fixture(&customer.to_string(),"community-bank","bank/method/one","reviewed");
                engine.write(format!(r#"{} insert $buyer isa tenant,has tenant_id "{buyer}",has name "Buyer",has status "active",has revision "initial";
                $c isa bill_customer,has bill_key "{customer}",has name "{label}",has bill_email "test@example.invalid",has bill_policy "customer",has bill_choice "automatic",has bill_provider "community-bank",has bill_bindings "{{}}",has bill_method_plugin "community-bank",has bill_payment_method "bank/method/one",has bill_consent "reviewed",has bill_revision "reviewed";(merchant:$t,buyer:$buyer,customer:$c) isa bill_account;{grant}
                $p isa bill_plan,has bill_key "{plan}",has name "{label}",has bill_amount 100,has bill_due_days 0,has bill_anchor {now},has bill_next {now},has bill_sequence 0,has bill_interval "once",has bill_status "active";(customer:$c,plan:$p) isa bill_plan_owner;(evidence:$grant,plan:$p) isa bill_consent_plan;fetch {{"id":$p.bill_key}};"#,engine.prefix())).await?;
            }
            fixtures.push((engine,bank,mail,email));
        }
        let mut catalog=server::automation::connectors::Catalog::default();catalog.register(Arc::new(BankConnector{tenants:fixtures.iter().map(|(e,b,m,_)|(e.merchant.clone(),(b.clone(),m.clone()))).collect()}))?;
        let vault=server::automation::vault::Vault::default();
        registry=registry.with_source(Arc::new(server::automation::managed::Managed{state:state.clone(),catalog:Arc::new(catalog.clone()),vault:vault.clone()}));
        catalog.register(Arc::new(server::automation::billing::Billing{state:state.clone(),registry:Arc::new(registry.clone())}))?;
        let catalog=Arc::new(catalog);
        eprintln!("Plugin automation billing workflows");
        for (engine,bank,mail,_) in &fixtures {
            let automation=server::automation::engine::Engine{store:server::automation::store::Store::new(state.clone(),&engine.merchant)?,catalog:catalog.clone(),vault:vault.clone()};
            let owners=engine.read(format!("match $u isa person,has email {};fetch {{'id':$u.identity_id}};",serde_json::to_string(&fixtures.iter().find(|(e,_,_,_)|e.merchant==engine.merchant).unwrap().3)?)).await?;let owner=owners[0]["id"].as_str().unwrap();
            automation.connect("community-bank","community-bank",serde_json::json!({"sandbox":true}),serde_json::Value::Null).await?;automation.verify_connection("community-bank").await?;
            automation.configure_billing(server::automation::managed::Settings{sandbox:true,default_financial:Some("community-bank".into()),notification:Some("community-bank".into()),reminder_days:7,allowed_test_recipients:vec!["test@example.invalid".into()]}).await?;
            automation.connect("billing","billing",serde_json::json!({}),serde_json::Value::Null).await?;automation.verify_connection("billing").await?;
            automation.publish(server::automation::model::Workflow{id:"billing".into(),revision:String::new(),name:"Complete billing".into(),enabled:true,steps:vec![server::automation::model::Step::Action{id:"cycle".into(),connection:"billing".into(),action:"tick".into(),input:serde_json::Value::Null}]},owner).await?;
            let run=automation.enqueue("billing","first",serde_json::Value::Null).await?;let done=automation.advance(run["id"].as_str().unwrap()).await?;ensure!(done["status"]=="completed","{done}");let report=&done["outputs"]["cycle"];ensure!(report["errors"]==0,"Plugin workflow failed: {report}");
            ensure!(report["issued"]==2&&report["notices"]==2,"Incomplete workflow: {report}");
            let paid={let invoices=bank.invoices.lock().unwrap();invoices.values().find(|i|i.state==InvoiceState::Paid).unwrap().invoice_id.clone()};
            automation.publish(server::automation::model::Workflow{id:"follow-up".into(),revision:String::new(),name:"Reconcile and notify".into(),enabled:true,steps:["reconcile","collect_due","notify"].into_iter().map(|action|server::automation::model::Step::Action{id:action.into(),connection:"billing".into(),action:action.into(),input:serde_json::json!({"invoice_id":paid})}).collect()},owner).await?;let follow=automation.enqueue("follow-up","paid-invoice",serde_json::Value::Null).await?;let fid=follow["id"].as_str().unwrap();for _ in 0..3{automation.advance(fid).await?;}ensure!(automation.store.get("run",fid).await?.unwrap().value["status"]=="completed","Individual billing actions failed");
            let repeated=engine.tick().await?;ensure!(repeated["issued"]==0&&repeated["notices"]==0&&repeated["errors"]==0,"Replay duplicated work: {repeated}");
            ensure!(bank.attempts.lock().unwrap().values().all(|n|*n==1),"Payment attempted twice");
            let messages=mail.messages.lock().unwrap();ensure!(messages.len()==2,"Wrong delivery count");
            ensure!(messages.values().any(|m|m.subject.contains("Payment received"))&&messages.values().any(|m|m.subject.contains("Payment reminder")),"Receipt/reminder missing");
        }
        eprintln!("Plugin HTTP boundary checks");
        let (a,_,_,email)=&fixtures[0];let (b,_,_,_)=&fixtures[1];
        let invoice=a.read(format!("{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key}};",a.prefix())).await?;
        ensure!(b.reconcile(invoice[0]["id"].as_str().unwrap()).await.is_err(),"Tenant crossed invoice boundary");
        std::env::set_var("BILLING_ENABLED","true");
        let (_,http)=server::app::build_with_plugins(state.clone(),"plugin-test-signing-secret-with-at-least-32-bytes".into(),Arc::new(registry))?;
        let router=server::app::router(http);
        let (status,login)=portal_call(&router,&a.merchant,"","/authentication",Some(serde_json::json!({"strategy":"local","email":email,"password":"plugin-owner-test-password"}))).await?;
        ensure!(status==200,"Custom-plugin tenant login failed: {status}");
        let token=login["accessToken"].as_str().ok_or_else(||anyhow::anyhow!("Missing login token"))?;
        ensure!(account_profile(&router,&a.merchant,token).await?.0==200,"Custom tenant profile failed");
        let (status,metadata)=portal_call(&router,&a.merchant,token,"/financial",None).await?;
        ensure!(status==200 && metadata[0]["sandbox"]==true,"Financial metadata exposed extra state: {status} {metadata}");
        ensure!(portal_call(&router,&a.merchant,"","/financial",None).await?.0==401,"Anonymous financial metadata allowed");
        ensure!(portal_call(&router,&b.merchant,token,"/financial",None).await?.0>=400,"Financial metadata crossed tenant membership");
        ensure!(portal_call(&router,&a.merchant,token,"/portal",None).await?.0>=400,"Retired profile handler is still active");
        for action in ["set_default","setup_card","unknown"] {
            let (status,_)=portal_call(&router,&a.merchant,token,"/financial",Some(serde_json::json!({"action":action}))).await?;
            ensure!(status==400,"Unsupported financial operation must be explicit: {action} {status}");
        }

        ensure!(portal_call(&router,&a.merchant,token,"/financial",Some(serde_json::json!({"action":"history","invoice_id":invoice[0]["id"]}))).await?.0==200,"Custom plugin portal dispatch failed");
        ensure!(account_profile(&router,&b.merchant,token).await?.0>=400,"Portal crossed tenant membership");
        Ok::<_,anyhow::Error>(())
    }.await;
    driver.databases().get(&name).await?.delete().await?;
    result
}

#[tokio::test]
#[ignore = "Development TypeDB, disposable legacy database; migration preserves references, consent and invoices"]
async fn explicit_migration_preserves_legacy_work_and_is_repeatable() -> Result<()> {
    use server::database;
    use typedb_driver::TransactionType;
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development credentials only"
    );
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let name = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    driver.databases().create(&name).await?;
    let result=async {
        let tx=driver.transaction(&name,TransactionType::Schema).await?;
        for source in [database::SCHEMA,database::FUNCTIONS,include_str!("../backend/pre-portal-retirement-policy.tql"),include_str!("../backend/legacy-billing.tql"),include_str!("../backend/pre-portal-retirement-account-links.tql")]{tx.query(source).await?;}tx.commit().await?;
        let tx=driver.transaction(&name,TransactionType::Write).await?;
        tx.query(r#"insert $t isa tenant,has tenant_id "seller",has name "Seller",has status "active",has revision "unchanged";$b isa tenant,has tenant_id "buyer",has name "Buyer",has status "active",has revision "initial";
        $c isa bill_customer,has bill_key "old-customer",has name "Customer",has bill_email "fixture@example.invalid",has bill_policy "customer",has bill_choice "automatic",has bill_provider "mercury",has bill_stripe_id "cus_kept",has bill_mercury_id "pending",has bill_payment_method "pm_kept",has bill_consent "approved",has bill_revision "approved",has bill_removed_method "pm_removed";(merchant:$t,buyer:$b,customer:$c) isa bill_account;
        $p isa bill_plan,has bill_key "old-plan",has name "Old plan",has bill_amount 100,has bill_due_days 7,has bill_anchor 1800000000,has bill_next 1800000000,has bill_sequence 1,has bill_interval "month",has bill_status "paused";(customer:$c,plan:$p) isa bill_plan_owner;
        $i isa bill_invoice,has bill_key "old-invoice",has name "Paid work",has bill_amount 100,has bill_anchor 1790000000,has bill_next 1790000000,has bill_sequence 0,has bill_choice "automatic",has bill_provider "stripe",has bill_status "paid",has bill_external_id "in_kept",has bill_url "https://invoice.stripe.com/i/kept",has bill_updated 1790000000,has bill_revision "attempted:approved";(plan:$p,invoice:$i) isa bill_invoice_owner;"#).await?;tx.commit().await?;
        ensure!(database::verify(&driver,&name).await.is_err(),"Old schema was accepted without migration");
        // Reproduce an interruption after additive schema expansion, before copying.
        let tx=driver.transaction(&name,TransactionType::Schema).await?;
        tx.query("define attribute bill_bindings @meta('dog.security.hidden','true'),value string;attribute bill_method_plugin @meta('dog.security.hidden','true'),value string;entity bill_customer owns bill_bindings @card(0..1),owns bill_method_plugin @card(0..1);").await?;tx.commit().await?;
        ensure!(database::verify(&driver,&name).await.is_err(),"Interrupted migration was accepted at startup");
        ensure!(server::plugin_migration::migrate(&driver,&name).await?==1,"Customer not migrated");
        ensure!(server::plugin_migration::migrate(&driver,&name).await?==0,"Migration is not repeatable");
        let schema=driver.databases().get(&name).await?.schema().await?;
        let compact:String=schema.chars().filter(|c|!c.is_whitespace()).collect();
        ensure!(compact.contains("ownsbill_stripe_id@card(0..1)")&&compact.contains("ownsbill_mercury_id@card(0..1)"),"Legacy references still required for new customers");
        let tx=driver.transaction(&name,TransactionType::Read).await?;
        let rows=database::documents(&tx,"match $c isa bill_customer;fetch {'bindings':$c.bill_bindings,'method_plugin':$c.bill_method_plugin,'choice':$c.bill_choice,'consent':$c.bill_consent,'revision':$c.bill_revision,'removed':[$c.bill_removed_method]};").await?;
        let row=&rows[0];let bindings:HashMap<String,String>=serde_json::from_str(row["bindings"].as_str().unwrap())?;
        ensure!(bindings["stripe"]=="cus_kept"&&bindings["mercury"]=="pending","External references lost");
        ensure!(row["choice"]=="automatic"&&row["consent"]=="approved"&&row["revision"]=="approved"&&row["method_plugin"]=="stripe","Consent or selected method changed");
        ensure!(row["removed"].as_array().unwrap().contains(&serde_json::json!("stripe:pm_removed")),"Removal protection lost");
        let invoices=database::documents(&tx,"match $i isa bill_invoice;fetch {'id':$i.bill_key,'external':$i.bill_external_id,'status':$i.bill_status};").await?;
        ensure!(invoices.len()==1&&invoices[0]["id"]=="old-invoice"&&invoices[0]["external"]=="in_kept"&&invoices[0]["status"]=="paid","Invoice history changed");
        tx.close().await?;Ok::<_,anyhow::Error>(())
    }.await;
    driver.databases().get(&name).await?.delete().await?;
    result
}

async fn portal_call(
    router: &axum::Router,
    tenant: &str,
    token: &str,
    path: &str,
    data: Option<serde_json::Value>,
) -> Result<(u16, serde_json::Value)> {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
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
    eprintln!("Plugin HTTP request {path}");
    let response = router
        .clone()
        .oneshot(request.body(axum::body::Body::from(
            data.map(|v| v.to_string()).unwrap_or_default(),
        ))?)
        .await
        .with_context(|| format!("Plugin HTTP {path}"))?;
    let status = response.status().as_u16();
    eprintln!("Plugin HTTP response {path}: {status}");
    let bytes = axum::body::to_bytes(response.into_body(), 1000000).await?;
    Ok((
        status,
        if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        },
    ))
}

#[path = "../support/mod.rs"]
mod support;

async fn account_profile(
    router: &axum::Router,
    tenant: &str,
    token: &str,
) -> Result<(u16, serde_json::Value)> {
    let (status,body)=portal_call(router,tenant,token,"/data",Some(serde_json::json!({"query":include_str!("../../public/queries/data/account-profile.tql")}))).await?;
    if status != 200 {
        return Ok((status, body));
    }
    let answers = body["ok"]["answers"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing account answers: {body}"))?;
    Ok((
        status,
        serde_json::json!(answers
            .iter()
            .map(|a| a["data"].clone())
            .collect::<Vec<_>>()),
    ))
}

#[tokio::test]
#[ignore = "Real development TypeDB; disposable graph, no payment or mail calls"]
async fn account_discovery_and_financial_metadata_enforce_tenant_scope() -> Result<()> {
    let _environment = environment::Environment::capture(&["BILLING_ENABLED"]);
    use server::{billing::engine::Engine, database, typedb::TypeDBState};
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development only"
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
    let name = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Disposable discovery database: {name}");
    database::install(&driver, &name).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: name.clone(),
    });
    let result=async {
        let first=uuid::Uuid::new_v4().to_string();let second=uuid::Uuid::new_v4().to_string();let identity=uuid::Uuid::new_v4().to_string();let email="discovery@example.invalid";
        let mut registry=TenantRegistry::default();
        registry.register(TenantRuntime::new(first.clone(),true))?;
        registry.register(TenantRuntime::new(second.clone(),true))?;
        let a=Engine::new(state.clone(),&first,registry.get(&first)?)?;
        let b=Engine::new(state.clone(),&second,registry.get(&second)?)?;
        let link=server::auth::local_link("$u",&identity)?;
        server::auth::plugins::local::Local::configured(&name)?.import(&identity,email,&bcrypt::hash("plugin-owner-test-password",4)?,"initial")?;
        a.write(format!("insert $t isa tenant,has tenant_id {},has name 'A',has status 'active',has revision 'initial'; $other isa tenant,has tenant_id {},has name 'B',has status 'active',has revision 'initial';$u isa person,has identity_id {},has email {},has name 'Owner',has status 'active';{link}(tenant:$t,member:$u) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;fetch {{'id':$u.identity_id}};",serde_json::to_string(&first)?,serde_json::to_string(&second)?,serde_json::to_string(&identity)?,serde_json::to_string(email)?)).await?;
        std::env::set_var("BILLING_ENABLED","true");
        let (_,http)=server::app::build_with_plugins(state.clone(),"discovery-test-signing-secret-long-enough".into(),Arc::new(registry))?;
        let router=server::app::router(http);
        let (status,login)=portal_call(&router,&first,"","/authentication",Some(serde_json::json!({"strategy":"local","email":email,"password":"plugin-owner-test-password"}))).await?;
        ensure!(status==200,"Discovery login: {status} {login}");
        let token=login["accessToken"].as_str().unwrap();
        let (status,profile)=account_profile(&router,&first,token).await?;
        ensure!(status==200&&profile[0]["email"]==email&&profile[0]["companies"].as_array().is_some_and(|x|x.len()==1),"Initial discovery: {status} {profile}");
        let (status,metadata)=portal_call(&router,&first,token,"/financial",None).await?;
        ensure!(status==200&&metadata[0]["sandbox"]==true,"Financial metadata: {status} {metadata}");
        ensure!(portal_call(&router,&second,token,"/financial",None).await?.0>=400,"Foreign financial metadata accepted");
        ensure!(portal_call(&router,&first,"","/financial",None).await?.0==401,"Anonymous financial metadata accepted");
        ensure!(portal_call(&router,&first,token,"/portal",None).await?.0==404,"Old profile handler remains");
        ensure!(portal_call(&router,&first,token,"/financial",Some(serde_json::json!({"action":"unknown"}))).await?.0==400,"Unsupported operation did not return a clear error");
        for method in ["PUT","PATCH","DELETE"] {
            use tower::ServiceExt;
            let response=router.clone().oneshot(axum::http::Request::builder().uri("/financial").method(method).header("x-tenant-id",&first).header("authorization",format!("Bearer {token}")).body(axum::body::Body::empty())?).await?;
            ensure!(response.status()==axum::http::StatusCode::METHOD_NOT_ALLOWED,"Unsupported financial HTTP method: {method}");
        }

        // A person's company picker can span memberships without widening the
        // tenant scope of ordinary graph reads, and revocation takes effect now.
        a.write(format!("{} $u isa person,has email {}; $other isa tenant,has tenant_id {}; insert (tenant:$other,member:$u) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;fetch {{'id':$u.identity_id}};",a.prefix(),serde_json::to_string(email)?,serde_json::to_string(&b.merchant)?)).await?;
        let (status,profile)=account_profile(&router,&a.merchant,token).await?;
        ensure!(status==200 && profile[0]["companies"].as_array().is_some_and(|c|c.len()==2),"Own second company was not discoverable: {profile}");
        let (status,foreign)=portal_call(&router,&a.merchant,token,"/data",Some(serde_json::json!({"query":format!("match $t isa tenant,has tenant_id {};fetch {{'id':$t.tenant_id}};",serde_json::to_string(&b.merchant)?)}))).await?;
        ensure!(status==200 && foreign["ok"]["answers"].as_array().is_some_and(Vec::is_empty),"Company discovery widened tenant data access: {foreign}");
        a.write(format!("{} $u isa person,has email {}; $other isa tenant,has tenant_id {}; $m isa membership,links(tenant:$other,member:$u); select $t,$m;distinct;update $m has end_at 2025-01-01T00:00:00;fetch {{'ended':true}};",a.prefix(),serde_json::to_string(email)?,serde_json::to_string(&b.merchant)?)).await?;
        let (_,profile)=account_profile(&router,&a.merchant,token).await?;
        ensure!(profile[0]["companies"].as_array().is_some_and(|c|c.len()==1),"Company discovery retained revoked membership: {profile}");
        ensure!(account_profile(&router,&b.merchant,token).await?.0>=400,"Revoked company scope still accepted");


        Ok::<_,anyhow::Error>(())
    }.await;
    driver.databases().get(&name).await?.delete().await?;
    result
}

mod retirement;

#[path = "../support/payment_fixture.rs"]
mod payment_fixture;

mod consent_migration;

#[path = "../support/environment.rs"]
mod environment;
