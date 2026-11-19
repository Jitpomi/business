//! Real gateway + dog-auth + scoped TypeDB + dog-queue with independent synthetic plugins.
use super::*;
use serde_json::{json, Value};
use server::{
    automation::{
        connectors::{Catalog, Connector, Invocation},
        model::ActionSpec,
    },
    typedb::TypeDBState,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[derive(Default)]
struct CheckoutBank {
    base: Bank,
    invoice_calls: Mutex<HashMap<String, usize>>,
    refunds: Mutex<HashMap<String, RefundObservation>>,
    submissions: AtomicUsize,
    lose_ack: AtomicBool,
}
#[async_trait]
impl FinancialPlugin for CheckoutBank {
    fn currencies(&self) -> &[&str] {
        &["USD", "JPY"]
    }
    fn method_currencies(&self, _: &SavedMethod) -> &[&str] {
        self.currencies()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_customers: true,
            automatic_collection: true,
            saved_methods: true,
            hosted_checkout: true,
            inline_payment: true,
            refunds: true,
            ..Default::default()
        }
    }
    fn ready(&self) -> Result<()> {
        Ok(())
    }
    async fn create_customer(&self, k: &str, n: &str, e: &str) -> Result<String> {
        self.base.create_customer(k, n, e).await
    }
    async fn create_invoice(&self, r: &InvoiceRequest) -> Result<String> {
        *self
            .invoice_calls
            .lock()
            .unwrap()
            .entry(r.id.clone())
            .or_default() += 1;
        self.base.create_invoice(r).await
    }
    async fn finalize_invoice(&self, r: &InvoiceRequest, id: &str) -> Result<InvoiceObservation> {
        self.base.finalize_invoice(r, id).await
    }
    async fn observe_invoice(&self, id: &str) -> Result<InvoiceObservation> {
        self.base.observe_invoice(id).await
    }
    async fn payment_form(&self, id: &str) -> Result<server::finance::PaymentPresentation> {
        let invoice = self.base.observe_invoice(id).await?;
        Ok(server::finance::PaymentPresentation {
            payable_minor: invoice.amount_minor,
            invoice,
            form: server::finance::PaymentForm::StripeElement {
                publishable_key: "pk_test_synthetic_gateway123".into(),
                stripe_account: None,
                client_secret: "pi_synthetic123_secret_synthetic123".into(),
            },
        })
    }
    async fn method(&self, c: &str, id: &str) -> Result<SavedMethod> {
        self.base.method(c, id).await
    }
    async fn methods(&self, c: &str) -> Result<Vec<SavedMethod>> {
        self.base.methods(c).await
    }
    async fn setup_method(
        &self,
        c: &str,
        l: &str,
        bank: bool,
        url: &str,
        key: &str,
    ) -> Result<String> {
        let target = reqwest::Url::parse(url)?;
        let context = target
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        ensure!(
            target.path() == "/checkout/"
                && context
                    .get("merchant")
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
                && context.get("plugin").is_some_and(|v| v == "bank"),
            "Saved-method return lost checkout context"
        );
        self.base.setup_method(c, l, bank, url, key).await
    }
    async fn confirm_method(&self, c: &str, l: &str, session: &str) -> Result<String> {
        self.base.confirm_method(c, l, session).await
    }
    async fn collect(&self, id: &str, key: &str, method: &str) -> Result<()> {
        self.base.collect(id, key, method).await
    }
    async fn refund(&self, invoice: &str, key: &str, amount: i64) -> Result<RefundObservation> {
        self.submissions.fetch_add(1, Ordering::SeqCst);
        let i = self.base.observe_invoice(invoice).await?;
        ensure!(i.state == InvoiceState::Paid);
        let value = RefundObservation {
            external_id: format!("refund-{key}"),
            request_id: key.into(),
            invoice_id: invoice.into(),
            customer: i.customer,
            amount_minor: amount,
            currency: i.currency,
            state: RefundState::Succeeded,
        };
        self.refunds
            .lock()
            .unwrap()
            .insert(key.into(), value.clone());
        ensure!(
            !self.lose_ack.swap(false, Ordering::SeqCst),
            "Synthetic lost refund acknowledgement"
        );
        Ok(value)
    }
    async fn find_refund(&self, _invoice: &str, key: &str) -> Result<Option<RefundObservation>> {
        Ok(self.refunds.lock().unwrap().get(key).cloned())
    }
}
struct CheckoutConnector {
    banks: HashMap<String, (Arc<CheckoutBank>, Arc<Mail>)>,
}
#[async_trait]
impl Connector for CheckoutConnector {
    fn name(&self) -> &str {
        "checkout-bank"
    }
    fn actions(&self) -> Vec<ActionSpec> {
        vec![]
    }
    fn validate(&self, c: &Value, s: &Value) -> Result<()> {
        ensure!(c == &json!({"sandbox":true}) && s.is_null());
        Ok(())
    }
    async fn verify(&self, c: &Value, s: &Value) -> Result<()> {
        self.validate(c, s)
    }
    async fn execute(&self, _: &Invocation, _: &str, _: Value) -> Result<Value> {
        anyhow::bail!(
            "This connector exposes financial and notification capabilities, not generic actions"
        )
    }
    fn financial(&self, c: &Invocation) -> Result<Option<Arc<dyn FinancialPlugin>>> {
        Ok(Some(
            self.banks
                .get(&c.tenant)
                .context("Foreign tenant connection")?
                .0
                .clone(),
        ))
    }
    fn notification(&self, c: &Invocation) -> Result<Option<Arc<dyn NotificationPlugin>>> {
        Ok(Some(
            self.banks
                .get(&c.tenant)
                .context("Foreign tenant notification")?
                .1
                .clone(),
        ))
    }
}
async fn bridge(path: &std::path::Path, phase: &str) -> Result<Value> {
    let root = std::env::var("BUSINESS_WEBSITE_DIR")?;
    let file = path.to_owned();
    let phase = phase.to_owned();
    tokio::task::spawn_blocking(move || {
        let out = std::process::Command::new("node")
            .arg(std::path::Path::new(&root).join("portal/checkout.integration.mjs"))
            .arg(file)
            .arg(&phase)
            .output()?;
        ensure!(
            out.status.success(),
            "Gateway {phase} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(serde_json::from_slice(&out.stdout)?)
    })
    .await?
}
#[tokio::test]
#[ignore = "Real development TypeDB and actual website gateway; isolated database and synthetic provider effects"]
async fn checkout_gateway_queue_payment_and_refund_recovery() -> Result<()> {
    checkout_gateway_scenario(Scenario::Full).await
}
#[tokio::test]
#[ignore = "Real TypeDB and gateway; mixed checkout with independent synthetic payment and notification plugins"]
async fn mixed_checkout_gateway_queue_payment_and_refund_recovery() -> Result<()> {
    checkout_gateway_scenario(Scenario::Mixed).await
}
#[tokio::test]
#[ignore = "Real TypeDB, dog-auth and website gateway; synthetic invoice form, no real provider credentials"]
async fn inline_checkout_gateway_authorization() -> Result<()> {
    checkout_gateway_scenario(Scenario::Inline).await
}
#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Full,
    Mixed,
    Inline,
}
async fn checkout_gateway_scenario(scenario: Scenario) -> Result<()> {
    let _environment = environment::Environment::capture(&[
        "BILLING_ENABLED",
        "AUTOMATION_ENABLED",
        "PORTAL_ORIGIN_SECRET",
        "AUTH_LOCAL_ENABLED",
    ]);
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    std::env::set_var("BILLING_ENABLED", "true");
    std::env::set_var("AUTOMATION_ENABLED", "true");
    std::env::remove_var("PORTAL_ORIGIN_SECRET");
    let driver = Arc::new(
        test_driver::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Checkout end-to-end fixture: {db}");
    server::database::install(&driver, &db).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: db.clone(),
    });
    let result=async{
  let merchant=uuid::Uuid::new_v4().to_string();let buyer=uuid::Uuid::new_v4().to_string();let outsider=uuid::Uuid::new_v4().to_string();let customer=uuid::Uuid::new_v4().to_string();
  let bank=Arc::new(CheckoutBank::default());bank.lose_ack.store(true,Ordering::SeqCst);let mail=Arc::new(Mail::default());
  let mut catalog=Catalog::default();catalog.register(Arc::new(CheckoutConnector{banks:[(merchant.clone(),(bank.clone(),mail.clone()))].into_iter().collect()}))?;
  let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
  let mut owner=String::new();
  for (name,tenant) in [("seller",&merchant),("buyer",&buyer),("outsider",&outsider)] {
   let identity=uuid::Uuid::new_v4().to_string();if name=="seller"{owner=identity.clone();}
   let email=format!("{name}@checkout.example.invalid");let hash=bcrypt::hash("checkout-integration-password",4)?;
   server::auth::plugins::local::Local::configured(&db)?.import(&identity,&email,&hash,"initial")?;
   tx.query(format!("insert $t isa tenant,has tenant_id '{tenant}',has name '{name}',has status 'active',has revision 'initial';$p isa person,has identity_id '{identity}',has name '{name}',has email '{email}',has status 'active';{}(tenant:$t,member:$p) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;",server::auth::local_link("$p",&identity)?)).await?;
  }
  tx.query(format!(r#"match $m isa tenant,has tenant_id "{merchant}";$b isa tenant,has tenant_id "{buyer}";insert $c isa bill_customer,has bill_key "{customer}",has name "Checkout buyer",has bill_email "test@example.invalid",has bill_policy "customer",has bill_choice "manual",has bill_provider "bank",has bill_bindings "{{}}",has bill_method_plugin "",has bill_payment_method "",has bill_consent "",has bill_revision "initial";(merchant:$m,buyer:$b,customer:$c) isa bill_account;"#)).await?;tx.commit().await?;
  let config_engine=server::automation::engine::Engine{store:server::automation::store::Store::new(state.clone(),&merchant)?.for_owner(&owner)?,catalog:Arc::new(catalog.clone()),vault:Default::default()};
  config_engine.connect("bank","checkout-bank",json!({"sandbox":true}),Value::Null).await?;config_engine.verify_connection("bank").await?;
  config_engine.configure_billing(server::automation::managed::Settings{sandbox:true,default_financial:Some("bank".into()),notification:Some("bank".into()),reminder_days:7,allowed_test_recipients:vec!["test@example.invalid".into()]}).await?;
  let (app,http)=server::app::build_with_connectors(state.clone(),"checkout-test-signing-secret-at-least-32-bytes".into(),Arc::new(TenantRegistry::default()),catalog)?;
  let template:Arc<server::automation::engine::Engine>=app.get("automation.engine").context("Queue template")?;
  let registry:Arc<TenantRegistry>=app.get("tenant.plugins").context("Tenant registry")?;
  server::billing::engine::Engine::new(state.clone(),&merchant,registry.resolve(&merchant).await?)?.refresh_commerce_plugins().await.context("Initial plugin currency attestation")?;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await?;let base=format!("http://{}",listener.local_addr()?);
  let server=tokio::spawn(async move{axum::serve(listener,server::app::router(http)).await.unwrap();});
  struct Listener(tokio::task::JoinHandle<()>);impl Drop for Listener{fn drop(&mut self){self.0.abort();}}let _listener=Listener(server);
  let directory=tempfile::tempdir()?;let config_file=directory.path().join("checkout.json");
  std::fs::write(&config_file,serde_json::to_vec(&json!({"currency":std::env::var("CHECKOUT_TEST_CURRENCY").unwrap_or("USD".into()),"base":base,"merchant":merchant,"customer":customer,"seller":{"email":"seller@checkout.example.invalid"},"buyer":{"email":"buyer@checkout.example.invalid"},"outsider":{"email":"outsider@checkout.example.invalid"}}))?)?;
  let worker=server::background::spawn_all((*template).clone());
  if scenario==Scenario::Mixed {worker.shutdown().await?;bridge(&config_file,"mixed-prepare").await?;return exercise_mixed_gateway(&config_file,&template,&bank,&mail).await;}
  let test=async{
   eprintln!("Gateway catalogue, quote, confirmation and company isolation");let created=bridge(&config_file,"create").await?;let order=created["order"].as_str().context("Order id")?;
   eprintln!("Awaiting dog-queue invoice execution for {order}");
   let invoice=tokio::time::timeout(std::time::Duration::from_secs(240),async{loop{let result=bridge(&config_file,"status").await?;let invoices=result["last"]["plans"].as_array().into_iter().flatten().flat_map(|p|p["invoices"].as_array().into_iter().flatten()).collect::<Vec<_>>();if let Some(i)=invoices.first(){if i["status"]=="open"{break Ok::<_,anyhow::Error>(i["id"].as_str().unwrap().to_owned());}}tokio::time::sleep(std::time::Duration::from_secs(5)).await;}}).await??;
   if scenario==Scenario::Inline {
    let proof=bridge(&config_file,"status").await?;ensure!(proof["inline_verified"]==true,"Inline gateway authorization was not exercised");
    let records=server::database::snapshot::records(&driver,&db).await?;
    ensure!(!serde_json::to_string(&records)?.contains("pi_synthetic123_secret_synthetic123"),"Provider confirmation secret entered TypeDB");
    ensure!(bank.invoice_calls.lock().unwrap().values().all(|n|*n==1),"Form read created another invoice");
    return Ok(());
   }
   let external=bank.base.invoices.lock().unwrap().iter().find(|(_,v)|v.invoice_id==invoice).map(|(k,_)|k.clone()).context("Provider invoice")?;
   let native_engine=server::billing::engine::Engine::new(state.clone(),&merchant,registry.resolve(&merchant).await?)?;
   let mut mismatched=bank.base.observe_invoice(&external).await?;let expected_currency=std::env::var("CHECKOUT_TEST_CURRENCY").unwrap_or("USD".into());ensure!(mismatched.currency==expected_currency);
   mismatched.currency=if expected_currency=="JPY" {"USD".into()} else {"JPY".into()};mismatched.state=InvoiceState::Paid;
   ensure!(native_engine.apply_observation(&invoice,"bank",&mismatched).await.is_err(),"Wrong-currency observation marked an invoice paid");

   bank.base.invoices.lock().unwrap().get_mut(&external).unwrap().state=InvoiceState::Processing;
   tokio::time::timeout(std::time::Duration::from_secs(180),async{loop{let r=bridge(&config_file,"status").await?;if r["last"]["plans"][0]["invoices"][0]["status"]=="processing"{break Ok::<_,anyhow::Error>(());}tokio::time::sleep(std::time::Duration::from_secs(5)).await;}}).await??;
   ensure!(!mail.messages.lock().unwrap().values().any(|m|m.subject.contains("Payment received")),"Processing generated a receipt");
   bank.base.invoices.lock().unwrap().get_mut(&external).unwrap().state=InvoiceState::Paid;
   tokio::time::timeout(std::time::Duration::from_secs(180),async{loop{let r=bridge(&config_file,"status").await?;if r["last"]["plans"][0]["invoices"][0]["status"]=="paid"&&mail.messages.lock().unwrap().values().filter(|m|m.subject.contains("Payment received")).count()==1{break Ok::<_,anyhow::Error>(());}tokio::time::sleep(std::time::Duration::from_secs(5)).await;}}).await??;
   eprintln!("Gateway refund authorization and lost-acknowledgement recovery");bridge(&config_file,"refund").await?;
   tokio::time::timeout(std::time::Duration::from_secs(240),async{loop{let status=bridge(&config_file,"status").await?;if status["last"]["plans"][0]["invoices"][0]["refunds"][0]["status"]=="succeeded"{break Ok::<_,anyhow::Error>(());}tokio::time::sleep(std::time::Duration::from_secs(5)).await;}}).await??;
   bridge(&config_file,"verify-refund").await?;
   ensure!(bank.submissions.load(Ordering::SeqCst)==1,"Uncertain refund was submitted twice");
   ensure!(bank.base.invoices.lock().unwrap().len()==1,"Checkout created duplicate invoices");
   bridge(&config_file,"future-quotes").await?;
   tokio::time::timeout(std::time::Duration::from_secs(240),async{loop{let result=bridge(&config_file,"automatic").await?;if result["ready"]==true{break Ok::<_,anyhow::Error>(());}tokio::time::sleep(std::time::Duration::from_secs(5)).await;}}).await??;
   Ok::<_,anyhow::Error>(())
  }.await;
  worker.shutdown().await?;test?;
  if scenario==Scenario::Inline {return Ok::<_,anyhow::Error>(());}
  eprintln!("Native logical-clock instalment and recurring-donation collection");
  let engine=server::billing::engine::Engine::new(state.clone(),&merchant,registry.resolve(&merchant).await?)?;
  let future=server::clock::unix_seconds()+95*86400;
  for _ in 0..8{if engine.enqueue_due(future).await?==0{break;}}
  let invoices=engine.read(format!("{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{'id':$i.bill_key,'status':$i.bill_status,'external':$i.bill_external_id}};",engine.prefix())).await?;
  for i in invoices{if i["status"]=="queued"{engine.issue(i["id"].as_str().unwrap()).await?;}engine.collect_due(i["id"].as_str().unwrap(),future).await?;}
  bridge(&config_file,"verify-plans").await?;
  ensure!(bank.invoice_calls.lock().unwrap().values().all(|n|*n==1),"A payment invoice was submitted twice");
  // Restart uses the same durable queue and business journal; no provider write is replayed.
  let worker=server::background::spawn_all((*template).clone());tokio::time::sleep(std::time::Duration::from_secs(5)).await;worker.shutdown().await?;
  ensure!(bank.submissions.load(Ordering::SeqCst)==1&&bank.invoice_calls.lock().unwrap().values().all(|n|*n==1));
  ensure!(bank.base.attempts.lock().unwrap().values().all(|n|*n==1),"A payment collection was submitted twice");
  ensure!(mail.calls.lock().unwrap().values().all(|n|*n==1),"A notification was submitted twice");
  {let messages=mail.messages.lock().unwrap();ensure!(messages.values().filter(|m|m.subject.contains("Payment received")).count()==1,"Duplicate payment receipt");ensure!(messages.values().filter(|m|m.subject.contains("Refund confirmed")).count()==1,"Refund receipt missing or duplicated");
  }
  exercise_mixed_gateway(&config_file,&template,&bank,&mail).await?;


  Ok::<_,anyhow::Error>(())
 }.await;
    if result.is_ok() {
        driver.databases().get(&db).await?.delete().await?;
    } else {
        eprintln!("Retained checkout fixture: {db}");
    }
    result
}

async fn exercise_mixed_gateway(
    config_file: &std::path::Path,
    template: &Arc<server::automation::engine::Engine>,
    bank: &Arc<CheckoutBank>,
    mail: &Arc<Mail>,
) -> Result<()> {
    eprintln!("Atomic mixed checkout, independent manual invoices and scoped partial refund");
    let submissions_before = bank.submissions.load(Ordering::SeqCst);
    bridge(config_file, "mixed-create").await?;
    let attempts_before = bank.base.attempts.lock().unwrap().clone();
    let mixed_worker = server::background::spawn_all((**template).clone());
    let mixed_test = async {
        let groups = tokio::time::timeout(std::time::Duration::from_secs(240), async {
            loop {
                let r = bridge(config_file, "mixed-status").await?;
                let groups = r["mixed_last"]["groups"]
                    .as_array()
                    .context("Mixed groups")?;
                if groups.len() == 3
                    && groups.iter().all(|g| {
                        g["plans"].as_array().is_some_and(|p| p.len() == 1)
                            && g["plans"][0]["invoices"]
                                .as_array()
                                .is_some_and(|i| i.len() == 1)
                            && g["plans"][0]["invoices"][0]["status"] == "open"
                    })
                {
                    break Ok::<_, anyhow::Error>(groups.clone());
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        })
        .await??;
        let once_invoice = groups.iter().find(|g| g["mode"] == "once").unwrap()["plans"][0]
            ["invoices"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let other_invoices = groups
            .iter()
            .filter(|g| g["mode"] != "once")
            .map(|g| {
                g["plans"][0]["invoices"][0]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        for group in groups {
            let invoice = group["plans"][0]["invoices"][0]["id"]
                .as_str()
                .context("Mixed invoice")?;
            ensure!(
                group["plans"][0]["choice"] == "manual",
                "Checkout acceptance silently authorized automatic collection"
            );
            let mut all = bank.base.invoices.lock().unwrap();
            let row = all
                .values_mut()
                .find(|v| v.invoice_id == invoice)
                .context("Mixed provider invoice")?;
            row.state = match group["interval"].as_str().unwrap() {
                "once" => InvoiceState::Paid,
                "month" => InvoiceState::Processing,
                _ => InvoiceState::Open,
            };
        }
        tokio::time::timeout(std::time::Duration::from_secs(240), async {
            loop {
                let r = bridge(config_file, "mixed-status").await?;
                let groups = r["mixed_last"]["groups"].as_array().unwrap();
                let ready = groups.iter().all(|g| {
                    g["plans"][0]["invoices"][0]["status"]
                        == match g["interval"].as_str().unwrap() {
                            "once" => "paid",
                            "month" => "processing",
                            _ => "open",
                        }
                });
                let received = {
                    let messages = mail.messages.lock().unwrap();
                    messages.contains_key(&format!("{once_invoice}-receipt"))
                        && other_invoices
                            .iter()
                            .all(|id| !messages.contains_key(&format!("{id}-receipt")))
                };
                if ready && received {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        })
        .await??;
        ensure!(
            *bank.base.attempts.lock().unwrap() == attempts_before,
            "Mixed checkout collected without scoped consent"
        );
        bridge(config_file, "mixed-refund").await?;
        tokio::time::timeout(std::time::Duration::from_secs(240), async {
            loop {
                let r = bridge(config_file, "mixed-status").await?;
                let once = r["mixed_last"]["groups"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|g| g["mode"] == "once")
                    .unwrap();
                let refunds = &once["plans"][0]["invoices"][0]["refunds"];
                if refunds.as_array().is_some_and(|r| r.len() == 1)
                    && refunds[0]["status"] == "succeeded"
                    && refunds[0]["id"].as_str().is_some_and(|id| {
                        mail.messages
                            .lock()
                            .unwrap()
                            .contains_key(&format!("{id}-refund-receipt"))
                    })
                {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        })
        .await??;
        bridge(config_file, "mixed-cancel").await?;
        ensure!(
            bank.submissions.load(Ordering::SeqCst) == submissions_before + 1
                && bank.invoice_calls.lock().unwrap().values().all(|n| *n == 1)
        );
        ensure!(
            mail.calls.lock().unwrap().values().all(|n| *n == 1),
            "Mixed checkout duplicated a receipt or reminder"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    mixed_worker.shutdown().await?;
    mixed_test?;
    Ok(())
}
