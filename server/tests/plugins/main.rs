//! Contract proof using a third-party-style bank and mail plugin, without vendor credentials.
use anyhow::{ensure, Result};
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
}
#[async_trait]
impl FinancialPlugin for Bank {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_customers: true,
            automatic_collection: true,
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
        p.methods("customer").await.is_err(),
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
    use server::{billing::engine::Engine, database, typedb::TypeDBState};
    use typedb_driver::TransactionType;
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
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
    database::install(&driver, &name).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: name.clone(),
    });
    let result=async{
        let mut fixtures=vec![];
        let mut registry=TenantRegistry::default();
        for _ in 0..2 {
            let tenant=uuid::Uuid::new_v4().to_string();let bank=Arc::new(Bank::default());let mail=Arc::new(Mail::default());
            let mut runtime=TenantRuntime::new(tenant.clone(),true);runtime.register("community-bank",bank.clone())?;runtime.default_financial=Some("community-bank".into());runtime.notification=Some(mail.clone());runtime.allowed_test_recipients.push("test@example.invalid".into());
            registry.register(runtime.clone())?;
            let engine=Engine::new(state.clone(),&tenant,Arc::new(runtime))?;
            let identity=uuid::Uuid::new_v4();let email=format!("{tenant}@example.invalid");let password_hash=bcrypt::hash("plugin-owner-test-password",4)?;
            let tx=driver.transaction(&name,TransactionType::Write).await?;
            tx.query(format!(r#"insert $t isa tenant,has tenant_id "{tenant}",has name "Plugin tenant",has status "active",has revision "initial";$u isa person,has identity_id "{identity}",has email "{email}",has name "Owner",has password_hash "{password_hash}",has credential_version "initial",has status "active";(tenant:$t,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;"#)).await?;tx.commit().await?;
            for label in ["success","decline"] {
                let customer=uuid::Uuid::new_v4();let buyer=uuid::Uuid::new_v4();let plan=uuid::Uuid::new_v4();let now=chrono::Utc::now().timestamp()-10;
                engine.write(format!(r#"{} insert $buyer isa tenant,has tenant_id "{buyer}",has name "Buyer",has status "active",has revision "initial";
                $c isa bill_customer,has bill_key "{customer}",has name "{label}",has bill_email "test@example.invalid",has bill_policy "customer",has bill_choice "automatic",has bill_provider "community-bank",has bill_bindings "{{}}",has bill_method_plugin "community-bank",has bill_payment_method "bank/method/one",has bill_consent "reviewed",has bill_revision "reviewed";(merchant:$t,buyer:$buyer,customer:$c) isa bill_account;
                $p isa bill_plan,has bill_key "{plan}",has name "{label}",has bill_amount 100,has bill_due_days 0,has bill_anchor {now},has bill_next {now},has bill_sequence 0,has bill_interval "once",has bill_status "active";(customer:$c,plan:$p) isa bill_plan_owner;fetch {{"id":$p.bill_key}};"#,engine.prefix())).await?;
            }
            fixtures.push((engine,bank,mail,email));
        }
        for (engine,bank,mail,_) in &fixtures {
            let report=engine.tick().await?;ensure!(report["errors"]==0,"Plugin workflow failed: {report}");
            ensure!(report["issued"]==2&&report["notices"]==2,"Incomplete workflow: {report}");
            let repeated=engine.tick().await?;ensure!(repeated["issued"]==0&&repeated["notices"]==0&&repeated["errors"]==0,"Replay duplicated work: {repeated}");
            ensure!(bank.attempts.lock().unwrap().values().all(|n|*n==1),"Payment attempted twice");
            let messages=mail.messages.lock().unwrap();ensure!(messages.len()==2,"Wrong delivery count");
            ensure!(messages.values().any(|m|m.subject.contains("Payment received"))&&messages.values().any(|m|m.subject.contains("Payment reminder")),"Receipt/reminder missing");
        }
        let (a,_,_,email)=&fixtures[0];let (b,_,_,_)=&fixtures[1];
        let invoice=a.read(format!("{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key}};",a.prefix())).await?;
        ensure!(b.reconcile(invoice[0]["id"].as_str().unwrap()).await.is_err(),"Tenant crossed invoice boundary");
        std::env::set_var("BILLING_ENABLED","true");
        let (_,http)=server::app::build_with_plugins(state.clone(),"plugin-test-signing-secret-with-at-least-32-bytes".into(),Arc::new(registry))?;
        let router=server::app::router(http);
        let (status,login)=portal_call(&router,&a.merchant,"","/authentication",Some(serde_json::json!({"strategy":"local","email":email,"password":"plugin-owner-test-password"}))).await?;
        ensure!(status==200,"Custom-plugin tenant login failed: {status}");
        let token=login["accessToken"].as_str().ok_or_else(||anyhow::anyhow!("Missing login token"))?;
        ensure!(portal_call(&router,&a.merchant,token,"/portal",None).await?.0==200,"Custom tenant profile failed");
        ensure!(portal_call(&router,&a.merchant,token,"/portal",Some(serde_json::json!({"action":"history","invoice_id":invoice[0]["id"]}))).await?.0==200,"Custom plugin portal dispatch failed");
        ensure!(portal_call(&router,&b.merchant,token,"/portal",None).await?.0>=400,"Portal crossed tenant membership");
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
        for source in [database::SCHEMA,database::FUNCTIONS,database::SCHEMA_POLICY,include_str!("../backend/legacy-billing.tql"),database::ACCOUNT_LINKS]{tx.query(source).await?;}tx.commit().await?;
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
    let bytes = axum::body::to_bytes(response.into_body(), 1000000).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}
