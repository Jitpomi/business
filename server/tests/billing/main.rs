//! Explicitly gated real sandbox workflow. No configured database is changed.
mod account_links;
mod financial;
mod mercury;
use anyhow::{ensure, Context, Result};
use serde_json::json;
use server::{
    database,
    plugins::builtin_transport::{pairs, Provider},
    typedb::TypeDBState,
};
use std::{collections::HashMap, sync::Arc};
use typedb_driver::TransactionType;

#[tokio::test]
#[ignore = "Requires approved development TypeDB, Stripe TEST key and approved Resend test recipient; creates only a disposable database and synthetic provider records"]
async fn scheduled_payment_receipt_and_decline_reminder() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development credentials required"
    );
    let secrets: HashMap<String, String> =
        dotenvy::from_path_iter(std::env::var("BILLING_TEST_ENV")?)?
            .collect::<std::result::Result<_, _>>()?;
    let token = secrets
        .get("STRIPE_SECRET_KEY")
        .context("Missing test key")?;
    ensure!(token.starts_with("sk_test_"), "Sandbox only");
    let merchant = uuid::Uuid::new_v4().to_string();
    std::env::set_var("BILLING_MERCHANT_ID", &merchant);
    std::env::set_var("BILLING_LIVE", "false");
    std::env::set_var("MERCURY_CONNECTIONS_JSON", json!([{"tenant_id":merchant,"identity_ids":["sandbox-fixture"],"environment":"sandbox","token":"readonly-fixture-never-sent","write":false}]).to_string());
    std::env::set_var("STRIPE_CONNECTIONS_JSON", json!([{"tenant_id":merchant,"identity_ids":["sandbox-fixture"],"environment":"test","token":token,"write":true}]).to_string());
    for key in ["RESEND_API_KEY", "BILLING_EMAIL_FROM", "BILLING_TEST_EMAIL"] {
        std::env::set_var(
            key,
            secrets
                .get(key)
                .context("Missing test mail configuration")?,
        );
    }
    std::env::set_var("BILLING_MAIL_ENABLED", "true");
    let recipient = std::env::var("BILLING_TEST_EMAIL")?;
    ensure!(
        recipient == "dev@jitpomi.com",
        "Only approved validation mailbox"
    );
    let address = std::env::var("TYPEDB_ADDR")?;
    let tls = std::env::var("TYPEDB_TLS").as_deref() != Ok("false");
    ensure!(
        tls || address.starts_with("127.0.0.1:"),
        "Plaintext is restricted to disposable loopback testing"
    );
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &address,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            tls,
        )
        .await?,
    );
    let name = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    database::install(&driver, &name).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: name.clone(),
    });
    let providers = server::plugins::configuration::transport(&merchant)?;
    std::env::set_var("TENANT_FINANCIAL_SETTINGS_JSON",json!([{"tenant_id":merchant,"default_financial":"stripe","webhook_secret":"whsec_synthetic_signature_test"}]).to_string());
    std::env::set_var("TENANT_NOTIFICATION_PLUGINS_JSON",json!([{"tenant_id":merchant,"plugin":"resend","token":std::env::var("RESEND_API_KEY")?,"from":std::env::var("BILLING_EMAIL_FROM")?,"allowed_test_recipients":[recipient]}]).to_string());
    let engine = server::plugins::configuration::engine(state, &merchant)?;
    let adapter = server::plugins::BuiltinFinancial {
        transport: providers.clone(),
        provider: Provider::Stripe,
        webhook_secret: None,
    };
    ensure!(
        providers.ready(Provider::Mercury).is_err(),
        "Read-only Mercury connection became writable"
    );
    let outcome=async {
        let now=chrono::Utc::now().timestamp();
        let tx=driver.transaction(&name,TransactionType::Write).await?;
        tx.query(format!(r#"insert $t isa tenant,has tenant_id "{merchant}",has name "TEST billing merchant",has status "active",has revision "initial";"#)).await?;
        tx.commit().await?;
        for (label,method) in [("success","pm_card_visa"),("decline","pm_card_chargeCustomerFail")] {
            let cid=uuid::Uuid::new_v4().to_string();
            let customer=providers.customer(Provider::Stripe,&cid,&format!("TEST ONLY {label}"),&recipient).await?;
            let saved=providers.stripe(reqwest::Method::POST,&format!("/payment_methods/{method}/attach"),&format!("fixture-{cid}"),&pairs(&[("customer",&customer)])).await?;
            let pm=saved["id"].as_str().context("Missing payment method")?;
            let plan=uuid::Uuid::new_v4().to_string();
            let bindings=server::billing::billing_schema::quoted(&json!({"stripe":customer}).to_string());
            engine.write(format!(r#"{} insert
                $buyer isa tenant,has tenant_id "{}",has name "TEST buyer {label}",has status "active",has revision "initial";
                $c isa bill_customer,has bill_key "{cid}",has name "TEST {label}",has bill_email "{recipient}",has bill_policy "customer",has bill_choice "automatic",has bill_provider "stripe",has bill_stripe_id "{customer}",has bill_mercury_id "",has bill_bindings {bindings},has bill_method_plugin "stripe",has bill_payment_method "{pm}",has bill_consent "fixture-consent",has bill_revision "fixture-consent";
                (merchant:$t,buyer:$buyer,customer:$c) isa bill_account;
                $p isa bill_plan,has bill_key "{plan}",has name "TEST ONLY {label} scheduled invoice",has bill_amount 100,has bill_due_days 0,has bill_anchor {now},has bill_next {now},has bill_sequence 0,has bill_interval "once",has bill_status "active";
                (customer:$c,plan:$p) isa bill_plan_owner;fetch {{"id":$p.bill_key}};"#,engine.prefix(),uuid::Uuid::new_v4())).await?;
        }
        // Not due yet; no provider effects. Overlapping due claims cannot duplicate occurrences.
        ensure!(engine.enqueue_due(now-1).await?==0,"Early invoice");
        let (a,b)=tokio::join!(engine.enqueue_due(now),engine.enqueue_due(now));
        ensure!(a.is_ok() || b.is_ok(),"Both scheduling claims failed: {a:?} {b:?}");
        ensure!(engine.enqueue_due(now).await?==0,"Schedule repeated");
        let rows=engine.read(format!(r#"{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{"id":$i.bill_key,"name":$i.name,"customer":$c.bill_key,"plan":$p.bill_key}};"#,engine.prefix())).await?;
        ensure!(rows.len()==2,"Duplicate or missing invoice: {}",rows.len());
        for row in &rows { engine.issue(row["id"].as_str().unwrap()).await?; }
        // Revocation and paused schedules must stop collection even without a changed revision.
        let sample=&rows[0]; let invoice=sample["id"].as_str().unwrap();
        engine.write(format!(r#"{} select $t,$c;distinct;update $c has bill_choice "manual";fetch {{"id":$c.bill_key}};"#,engine.invoice_match(invoice)?)).await?;
        engine.collect_due(invoice,now).await?;
        engine.write(format!(r#"{} select $t,$c,$p;distinct;update $c has bill_choice "automatic";$p has bill_status "paused";fetch {{"id":$c.bill_key}};"#,engine.invoice_match(invoice)?)).await?;
        engine.collect_due(invoice,now).await?;
        let binding=engine.read(format!(r#"{} fetch {{"external":$i.bill_external_id}};"#,engine.invoice_match(invoice)?)).await?;
        let observed=providers.observe(Provider::Stripe,binding[0]["external"].as_str().unwrap()).await?;
        ensure!(observed["attempt_count"]==0,"Revoked consent or paused schedule was charged");
        engine.write(format!(r#"{} select $t,$p;distinct;update $p has bill_status "finished";fetch {{"id":$p.bill_key}};"#,engine.invoice_match(invoice)?)).await?;
        let mut other=engine.clone();other.merchant=uuid::Uuid::new_v4().to_string();
        ensure!(other.reconcile(invoice).await.is_err(),"Cross-tenant invoice access");
        let mut wrong=observed.clone();wrong["total"]=json!(101);
        ensure!(engine.apply_observation(invoice,"stripe",&adapter.observation(&wrong)?).await.is_err(),"Amount mismatch accepted");
        let other_invoice=rows[1]["id"].as_str().unwrap();
        ensure!(engine.apply_observation(other_invoice,"stripe",&adapter.observation(&observed)?).await.is_err(),"Invoice substitution accepted");
        // Actual production tick handles both due collection and receipt/reminder delivery.
        let tick=engine.tick().await?;
        ensure!(tick["errors"]==0,"Tick failed: {tick}");
        let again=engine.tick().await?;
        ensure!(again["errors"]==0 && again["notices"]==0 && again["issued"]==0,"Retry was not idempotent: {again}");
        let evidence=engine.read(format!(r#"{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;(invoice:$i,notice:$n) isa bill_notice_owner;fetch {{"id":$i.bill_key,"name":$i.name,"status":$i.bill_status,"external":$i.bill_external_id,"notice":$n.bill_key,"delivery":$n.bill_status}};"#,engine.prefix())).await?;
        ensure!(evidence.len()==2,"Expected exactly receipt and reminder");
        for row in &evidence {
            let success=row["name"].as_str().unwrap().contains("success");
            ensure!(row["status"]==if success{"paid"}else{"open"},"Unexpected invoice outcome");
            ensure!(row["notice"].as_str().unwrap().ends_with(if success{"-receipt"}else{"-reminder-0"}),"Wrong notice");
            ensure!(row["delivery"].as_str().unwrap().starts_with("accepted:"),"Email not accepted");
            let observation=providers.observe(Provider::Stripe,row["external"].as_str().unwrap()).await?;
            ensure!(observation["auto_advance"]==false,"Hidden retries enabled");
            if !success { ensure!(observation["attempt_count"].as_u64()==Some(1),"Duplicate payment attempt"); }
        }
        // Exercise the actual HTTP ingress using signed raw provider events. Payment
        // state is fetched from Stripe, never taken from the supplied event payload.
        use tower::ServiceExt;
        use hmac::{Hmac,Mac};
        use sha2::Sha256;
        use axum::{body::Body,http::Request};
        std::env::set_var("BILLING_TICK_SECRET","sandbox-tick-secret-at-least-32-characters");
        std::env::set_var("STRIPE_WEBHOOK_SECRET","whsec_synthetic_signature_test");
        let router=server::billing::http::router(engine.clone())?;
        let denied=router.clone().oneshot(Request::post("/internal/billing/tick").body(Body::empty())?).await?;
        ensure!(denied.status()==404,"Removed scheduler endpoint still exists");
        let paid=evidence.iter().find(|r|r["status"]=="paid").unwrap();
        for (signed,live,account,expected) in [(false,false,None,401),(true,true,None,400),(true,false,Some("acct_foreign"),400),(true,false,None,200),(true,false,None,200)] {
            let body=json!({"id":"evt_fixture","type":"invoice.payment_succeeded","livemode":live,"account":account,"data":{"object":{"id":paid["external"],"status":"open","metadata":{"business_invoice":paid["id"]}}}}).to_string();
            let ts=chrono::Utc::now().timestamp();
            let mut mac=Hmac::<Sha256>::new_from_slice(b"whsec_synthetic_signature_test")?;
            mac.update(format!("{ts}.{body}").as_bytes());
            let digest=mac.finalize().into_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>();
            let header=if signed{format!("t={ts},v1={digest}")}else{"invalid".into()};
            let response=router.clone().oneshot(Request::post("/webhooks/stripe").header("stripe-signature",header).body(Body::from(body))?).await?;
            ensure!(response.status().as_u16()==expected,"Unexpected webhook response {}",response.status());
        }
        ensure!(engine.notify_invoice(paid["id"].as_str().unwrap()).await?==0,"Webhook replay duplicated receipt");
        // Imported historical invoices must never re-enter issuing, collection or notice delivery.
        let archived=evidence.iter().find(|r|r["status"]=="open").unwrap()["id"].as_str().unwrap();
        engine.write(format!(r#"{} select $t,$i;distinct;update $i has bill_status "queued";insert $n isa bill_notice,has bill_key "{archived}-migration-archive",has bill_status "imported-history",has bill_updated {now};(invoice:$i,notice:$n) isa bill_notice_owner;fetch {{"id":$n.bill_key}};"#,engine.invoice_match(archived)?)).await?;
        let archive_tick=engine.tick().await?;
        ensure!(archive_tick["issued"]==0 && archive_tick["reconciled"]==0 && archive_tick["notices"]==0 && archive_tick["errors"]==0,"Imported invoice was processed");
        ensure!(engine.notify_invoice(archived).await?==0,"Imported invoice produced a notice");
        financial::check(&engine).await?;
        account_links::check(&engine).await?;
        let report=json!({"test":"current-backend-full-sandbox-billing","tick":tick,"repeat_tick":again,"invoices":evidence,"security_checks":["tenant isolation","provider binding","consent revocation","paused schedule","unsigned webhook denied","wrong environment denied","wrong Connect account denied","signed webhook replay"]});
        if let Ok(path)=std::env::var("BILLING_TEST_REPORT") { std::fs::write(path,serde_json::to_string_pretty(&report)?)?; }
        println!("Verified: two scheduled invoices, successful payment + receipt, declined payment + reminder, repeated tick with no duplicate effects.");
        Ok::<_,anyhow::Error>(())
    }.await;
    // Delete only the exact UUID database created above; provider test objects remain auditable.
    driver.databases().get(&name).await?.delete().await?;
    outcome
}

/// Account and invoice state acceptance does not require payment or email keys.
#[tokio::test]
#[ignore = "Real development TypeDB; disposable database only; no provider calls or email"]
async fn account_and_invoice_workflow_without_provider_effects() -> Result<()> {
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
    let merchant = uuid::Uuid::new_v4().to_string();
    for (key, value) in [
        ("BILLING_MERCHANT_ID", merchant.as_str()),
        ("BILLING_ENABLED", "true"),
        ("BILLING_LIVE", "false"),
        ("BILLING_MAIL_ENABLED", "false"),
        ("STRIPE_CONNECTIONS_JSON", "[]"),
        ("MERCURY_CONNECTIONS_JSON", "[]"),
        (
            "BILLING_TICK_SECRET",
            "fixture-tick-secret-with-at-least-32-bytes",
        ),
    ] {
        std::env::set_var(key, value);
    }
    std::env::set_var("TENANT_NOTIFICATION_PLUGINS_JSON", "[]");
    std::env::set_var("TENANT_FINANCIAL_SETTINGS_JSON", "[]");
    let engine = server::plugins::configuration::engine(
        Arc::new(TypeDBState {
            driver: driver.clone(),
            database: name.clone(),
        }),
        &merchant,
    )?;
    database::install(&driver, &name).await?;
    let result = async {
        let tx = driver.transaction(&name, TransactionType::Write).await?;
        tx.query(format!(r#"insert $t isa tenant,has tenant_id "{merchant}",has name "TEST workflow",has status "active",has revision "initial";"#)).await?;
        tx.commit().await?;
        account_links::check(&engine).await?;
        let customer = engine.read(format!(r#"{} (merchant:$t,customer:$c) isa bill_account;$c has bill_email "new-links@example.com";fetch {{"id":$c.bill_key}};"#, engine.prefix())).await?;
        ensure!(customer.len() == 1, "Accepted customer missing");
        let customer = customer[0]["id"].as_str().context("Missing customer ID")?;
        let plan = uuid::Uuid::new_v4().to_string();
        let at = chrono::Utc::now().timestamp();
        engine.write(format!(r#"{} insert $p isa bill_plan,has bill_key "{plan}",has name "TEST one-time work",has bill_amount 500,has bill_due_days 7,has bill_anchor {at},has bill_next {at},has bill_sequence 0,has bill_interval "once",has bill_status "active";(customer:$c,plan:$p) isa bill_plan_owner;fetch {{"id":$p.bill_key}};"#,engine.customer_match(customer)?)).await?;
        ensure!(engine.enqueue_due(at - 1).await? == 0, "Invoice issued early");
        ensure!(engine.enqueue_due(at).await? == 1, "Due occurrence missing");
        ensure!(engine.enqueue_due(at).await? == 0, "Duplicate occurrence");
        let rows = engine.read(format!(r#"{} (customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{"id":$i.bill_key,"due":$i.bill_next,"status":$i.bill_status,"plan_status":$p.bill_status}};"#,engine.customer_match(customer)?)).await?;
        ensure!(rows.len() == 1 && rows[0]["plan_status"] == "finished", "One-time schedule did not finish");
        ensure!(server::billing::engine::number(&rows[0], "due")? == at + 7 * 86400, "Incorrect due date");
        let invoice = rows[0]["id"].as_str().context("Missing invoice")?;
        ensure!(engine.issue(invoice).await.is_err(), "Missing provider accepted");
        let stored = engine.read(format!(r#"{} fetch {{"status":$i.bill_status}};"#,engine.invoice_match(invoice)?)).await?;
        ensure!(stored[0]["status"] == "queued", "Missing provider consumed the invoice claim");
        // Seed a provider observation binding, without contacting any provider.
        let bindings=server::billing::billing_schema::quoted(&json!({"stripe":"cus_fixture"}).to_string());
        engine.write(format!(r#"{} select $t,$c,$i;distinct;update $c has bill_bindings {bindings};$i has bill_provider "stripe",has bill_external_id "in_fixture",has bill_status "open";fetch {{"id":$i.bill_key}};"#,engine.invoice_match(invoice)?)).await?;
        let observation=server::finance::InvoiceObservation{external_id:"in_fixture".into(),customer:"cus_fixture".into(),currency:"USD".into(),amount_minor:500,invoice_id:invoice.into(),state:server::finance::InvoiceState::Paid,payment_url:"https://invoice.stripe.com/i/fixture".into()};
        let mut wrong=observation.clone();wrong.amount_minor=501;
        ensure!(engine.apply_observation(invoice,"stripe",&wrong).await.is_err(),"Wrong amount accepted");
        engine.apply_observation(invoice,"stripe",&observation).await?;
        let mut stale=observation;stale.state=server::finance::InvoiceState::Open;
        engine.apply_observation(invoice,"stripe",&stale).await?;
        let stored = engine.read(format!(r#"{} fetch {{"status":$i.bill_status}};"#,engine.invoice_match(invoice)?)).await?;
        ensure!(stored[0]["status"] == "paid", "Stale observation reopened a paid invoice");
        ensure!(engine.notify_invoice(invoice).await? == 0, "Disabled mail attempted delivery");
        Ok::<_, anyhow::Error>(())
    }.await;
    driver.databases().get(&name).await?.delete().await?;
    result
}

#[path = "../support/mod.rs"]
mod support;
