//! Real Mercury sandbox, isolated billing graph, independent notification plugin.
use super::*;
#[tokio::test]
#[ignore = "Approved Mercury sandbox and test mailbox; creates a disposable DB and a synthetic USD 1 invoice; payment must be completed in sandbox UI"]
async fn mercury_manual_invoice_payment_receipt_and_reminder() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development credentials only"
    );
    let secrets: HashMap<String, String> =
        dotenvy::from_path_iter(std::env::var("BILLING_TEST_ENV")?)?
            .collect::<std::result::Result<_, _>>()?;
    let tenant = uuid::Uuid::new_v4().to_string();
    let recipient = secrets
        .get("BILLING_TEST_EMAIL")
        .context("Missing test recipient")?;
    ensure!(recipient == "dev@jitpomi.com", "Approved test mailbox only");
    std::env::set_var("STRIPE_CONNECTIONS_JSON", "[]");
    std::env::set_var("MERCURY_CONNECTIONS_JSON",json!([{"tenant_id":tenant,"identity_ids":["sandbox-fixture"],"environment":"sandbox","write":true,"token":secrets.get("MERCURY_SANDBOX_TOKEN").context("Missing Mercury sandbox token")?}]).to_string());
    std::env::set_var("TENANT_FINANCIAL_SETTINGS_JSON",json!([{"tenant_id":tenant,"sandbox":true,"default_financial":"mercury","mercury_destination":secrets.get("MERCURY_SANDBOX_ACCOUNT_ID").context("Missing sandbox destination")?,"mercury_card_enabled":false,"mercury_ach_debit_enabled":false}]).to_string());
    std::env::set_var("TENANT_NOTIFICATION_PLUGINS_JSON",json!([{"tenant_id":tenant,"plugin":"resend","token":secrets.get("RESEND_API_KEY").context("Missing notification key")?,"from":secrets.get("BILLING_EMAIL_FROM").context("Missing sender")?,"allowed_test_recipients":[recipient]}]).to_string());
    let transport = server::plugins::configuration::transport(&tenant)?;
    // Fail before any creation if this credential/network cannot access its destination.
    transport
        .mercury(
            reqwest::Method::GET,
            &format!("/account/{}", secrets["MERCURY_SANDBOX_ACCOUNT_ID"]),
            None,
        )
        .await
        .context("Mercury destination preflight")?;
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
    let engine = server::plugins::configuration::engine(
        Arc::new(TypeDBState {
            driver: driver.clone(),
            database: name.clone(),
        }),
        &tenant,
    )?;
    let result=async {
        let customer=uuid::Uuid::new_v4();let buyer=uuid::Uuid::new_v4();let plan=uuid::Uuid::new_v4();let now=chrono::Utc::now().timestamp();
        let tx=driver.transaction(&name,TransactionType::Write).await?;
        tx.query(format!(r#"insert $t isa tenant,has tenant_id "{tenant}",has name "TEST Mercury plugin",has status "active",has revision "initial";$b isa tenant,has tenant_id "{buyer}",has name "TEST buyer",has status "active",has revision "initial";$c isa bill_customer,has bill_key "{customer}",has name "TEST ONLY Mercury plugin",has bill_email "{recipient}",has bill_policy "manual",has bill_choice "manual",has bill_provider "mercury",has bill_bindings "{{}}",has bill_method_plugin "",has bill_payment_method "",has bill_consent "",has bill_revision "initial";(merchant:$t,buyer:$b,customer:$c) isa bill_account;$p isa bill_plan,has bill_key "{plan}",has name "TEST ONLY Mercury plugin invoice",has bill_amount 100,has bill_due_days 0,has bill_anchor {now},has bill_next {now},has bill_sequence 0,has bill_interval "once",has bill_status "active";(customer:$c,plan:$p) isa bill_plan_owner;"#)).await?;tx.commit().await?;
        ensure!(engine.enqueue_due(now).await?==1,"Invoice occurrence missing");
        let rows=engine.read(format!("{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key}};",engine.prefix())).await?;
        let id=rows[0]["id"].as_str().context("Missing invoice")?;
        engine.issue(id).await?;engine.reconcile(id).await?;
        ensure!(engine.notify_invoice(id).await?==1,"Initial reminder missing");
        ensure!(engine.notify_invoice(id).await?==0,"Duplicate reminder");
        let invoice=engine.read(format!("{} fetch {{'external':$i.bill_external_id,'url':$i.bill_url,'status':$i.bill_status}};",engine.invoice_match(id)?)).await?;
        ensure!(invoice[0]["status"]=="open","Expected unpaid invoice");
        println!("Mercury sandbox invoice ready: {}",invoice[0]["url"].as_str().unwrap_or(""));
        if let Ok(path)=std::env::var("BILLING_TEST_REPORT") {std::fs::write(path,serde_json::to_vec_pretty(&json!({"phase":"awaiting_sandbox_payment","database":name,"tenant":tenant,"invoice_id":id,"invoice":invoice[0]}))?)?;}
        ensure!(std::env::var("MERCURY_WAIT_FOR_PAYMENT").as_deref()==Ok("true"),"Invoice/reminder verified; set MERCURY_WAIT_FOR_PAYMENT=true to verify a real sandbox payment and receipt");
        let mut paid=false;
        for _ in 0..60 {
            engine.reconcile(id).await?;
            let row=engine.read(format!("{} fetch {{'status':$i.bill_status}};",engine.invoice_match(id)?)).await?;
            if row[0]["status"]=="paid" {paid=true;break;}
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
        ensure!(paid,"Sandbox payment not observed; receipt phase not verified");
        ensure!(engine.notify_invoice(id).await?==1,"Receipt missing");
        let repeated=engine.tick().await?;
        ensure!(repeated["errors"]==0&&repeated["notices"]==0&&repeated["issued"]==0,"Replay duplicated effects");
        if let Ok(path)=std::env::var("BILLING_TEST_REPORT") {std::fs::write(path,serde_json::to_vec_pretty(&json!({"phase":"passed","invoice":invoice[0],"payment":"paid_observed_from_mercury","reminder":"accepted_once","receipt":"accepted_once","repeat_tick":repeated}))?)?;}
        Ok::<_,anyhow::Error>(())
    }.await;
    driver.databases().get(&name).await?.delete().await?;
    result
}
