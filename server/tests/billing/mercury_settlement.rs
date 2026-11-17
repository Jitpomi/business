//! Actual bank settlement acceptance. Only dog-queue invokes reconciliation/mail.
//! The provider oracle is GET-only; a manual payment can never satisfy this test.
use super::{
    sandbox_support::{ok, save},
    *,
};
use server::automation::store::Store;
use std::{path::Path, time::Duration};

async fn native(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    query: String,
) -> Result<Vec<serde_json::Value>> {
    let value = ok(
        app,
        tenant,
        token,
        "/data",
        json!({"query":query}),
        Some("read"),
    )
    .await?;
    Ok(value["ok"]["answers"]
        .as_array()
        .context("Missing native answers")?
        .iter()
        .map(|r| r["data"].clone())
        .collect())
}

// Journal/binding facts are deliberately hidden from /data. Inspect them with
// the operator fixture connection and a fresh native owner/tenant predicate;
// never widen the public schema policies to accommodate an acceptance test.
async fn audit(store: &Store, body: &str) -> Result<Vec<serde_json::Value>> {
    let tx = store
        .state
        .driver
        .transaction(&store.state.database, TransactionType::Read)
        .await?;
    let rows = database::documents(&tx, &format!("{} {body}", store.prefix())).await?;
    tx.close().await?;
    Ok(rows)
}

pub(super) async fn check(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    store: &Store,
    root: &Path,
    saved: &serde_json::Value,
    secrets: &HashMap<String, String>,
) -> Result<()> {
    let invoice = std::env::var("MERCURY_BANK_INVOICE_ID")
        .context("Explicit existing bank-payment invoice required")?;
    uuid::Uuid::parse_str(&invoice)?;
    ensure!(
        saved["manual_invoice"] == invoice,
        "Select the preserved bank checkpoint, not the separate manually-paid case"
    );
    let budget = std::env::var("MERCURY_SETTLEMENT_WAIT_SECONDS")
        .unwrap_or_else(|_| "180".into())
        .parse::<u64>()?;
    ensure!(
        (150..=86400).contains(&budget),
        "Use an explicit bounded settlement observation window"
    );
    let schedule_id = format!("bank-observe-{}", uuid::Uuid::new_v4().simple());
    let campaign = root.join(&schedule_id);
    let mut directory = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(&campaign)?;
    save(
        &root.join("bank-auto-latest.json"),
        &json!({"directory":campaign,"schedule":schedule_id,"invoice":invoice}),
    )?;
    let root = campaign.as_path();
    let q = |v: &str| serde_json::to_string(v).unwrap();
    let invoice_query=format!("match $i isa bill_invoice,has bill_key {};fetch {{'id':$i.bill_key,'status':$i.bill_status,'plugin':$i.bill_provider,'amount':$i.bill_amount}};",q(&invoice));
    let scoped = store.for_owner(saved["owner"].as_str().context("Owner missing")?)?;
    let private_invoice=format!("(merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;$i has bill_key {};",q(&invoice));
    let notice_query=format!("{private_invoice}(invoice:$i,notice:$n) isa bill_notice_owner;$n has bill_key $key;select $n,$key;sort $key;fetch {{'id':$n.bill_key,'status':$n.bill_status}};");
    let initial = native(app, tenant, token, invoice_query.clone()).await?;
    ensure!(
        initial.len() == 1 && initial[0]["plugin"] == "mercury",
        "Tenant's Mercury invoice required"
    );
    let binding = audit(
        &scoped,
        &format!("{private_invoice}fetch {{'external':$i.bill_external_id}};"),
    )
    .await?;
    ensure!(binding.len() == 1, "Native tenant binding unavailable");
    let external = binding[0]["external"]
        .as_str()
        .context("Provider invoice missing")?;
    let oracle = server::plugins::builtin_transport::Providers::connected(
        false,
        None,
        Some(secrets["MERCURY_SANDBOX_TOKEN"].clone()),
        None,
        Some(secrets["MERCURY_SANDBOX_ACCOUNT_ID"].clone()),
    )?;
    let provider_before = oracle
        .mercury(
            reqwest::Method::GET,
            &format!("/ar/invoices/{external}"),
            None,
        )
        .await?;
    let payments_before = oracle
        .mercury(
            reqwest::Method::GET,
            &format!("/ar/invoices/{external}/payments"),
            None,
        )
        .await?;
    ensure!(
        provider_before["amount"].as_f64() == Some(1.0)
            && provider_before["currencyCode"] == "USD"
            && provider_before["invoiceNumber"] == format!("JIT-{invoice}")
            && server::billing::engine::number(&initial[0], "amount")? == 100,
        "Exact existing USD 1 bank invoice required"
    );
    ensure!(
        payments_before["payments"]
            .as_array()
            .context("Missing initial payments")?
            .iter()
            .all(|p| p["type"] != "manual"),
        "Manual payment records disqualify this test before scheduling"
    );
    let before = audit(&scoped, &notice_query).await?;
    save(
        &root.join("bank-auto-baseline.json"),
        &json!({"local":initial,"provider":provider_before,"payments":payments_before,"notices":before}),
    )?;
    let id = format!("bank-settlement-{invoice}");
    if store.get("workflow", &id).await?.is_none() {
        ok(app,tenant,token,"/automation",json!({"action":"publish","workflow":{"id":id,"revision":"","name":"Actual Mercury bank settlement acceptance","enabled":true,"steps":[{"kind":"action","id":"observe","connection":"billing","action":"reconcile","input":{"invoice_id":{"$ref":"input/invoice_id"}}},{"kind":"action","id":"receipt","connection":"billing","action":"notify","input":{"invoice_id":{"$ref":"input/invoice_id"}}}]}}),None).await?;
    }
    let published = store
        .get("workflow", &id)
        .await?
        .context("Published workflow missing")?;
    let steps = &published.value["workflow"]["steps"];
    ensure!(
        published.value["owner"] == saved["owner"]
            && published.value["workflow"]["enabled"] == true
            && steps
                == &json!([{"kind":"action","id":"observe","connection":"billing","action":"reconcile","input":{"invoice_id":{"$ref":"input/invoice_id"}}},{"kind":"action","id":"receipt","connection":"billing","action":"notify","input":{"invoice_id":{"$ref":"input/invoice_id"}}}]),
        "Existing acceptance workflow differs; retain it for review"
    );

    let schedule = |enabled: bool| json!({"action":"schedule","schedule":{"id":schedule_id,"workflow":id,"every_seconds":60,"next":server::automation::engine::now(),"sequence":0,"input":{"invoice_id":invoice,"acceptance_schedule":schedule_id},"enabled":enabled}});
    let prior = store
        .get("schedule", &schedule_id)
        .await?
        .and_then(|r| r.value["sequence"].as_u64())
        .unwrap_or(0);
    let completed_before = store
        .list("run", 1000)
        .await?
        .iter()
        .filter(|r| {
            r.value["workflow"]["id"] == id
                && r.value["input"]["acceptance_schedule"] == schedule_id
                && r.value["status"] == "completed"
        })
        .count();
    ok(app, tenant, token, "/automation", schedule(true), None).await?;
    let outcome=async {
        let deadline=tokio::time::Instant::now()+Duration::from_secs(budget);
        let mut first_receipt_sequence=None;
        let mut first_receipt_completed=None;
        let mut first_receipt_id:Option<String>=None;
        let mut last=serde_json::Value::Null;
        loop {
            let local=native(app,tenant,token,invoice_query.clone()).await?;
            ensure!(local.len()==1,"Invoice disappeared");
            let notices=audit(&scoped,&notice_query).await?;
            let receipts=notices.iter().filter(|n|n["id"]==format!("{invoice}-receipt")).collect::<Vec<_>>();
            ensure!(receipts.len()<=1,"Duplicate receipt claims");
            let raw=oracle.mercury(reqwest::Method::GET,&format!("/ar/invoices/{external}"),None).await?;
            let payments=oracle.mercury(reqwest::Method::GET,&format!("/ar/invoices/{external}/payments"),None).await?;
            let payment_rows=payments["payments"].as_array().context("Missing applied payments")?;
            ensure!(payment_rows.iter().all(|p|p["type"]!="manual"),"Manual payment override disqualifies actual bank settlement");
            let sequence=store.get("schedule",&schedule_id).await?.context("Schedule disappeared")?.value["sequence"].as_u64().context("Missing schedule cursor")?;
            let completed=store.list("run",1000).await?.iter().filter(|r|r.value["workflow"]["id"]==id && r.value["input"]["acceptance_schedule"]==schedule_id && r.value["status"]=="completed" && r.value["outputs"].get("observe").is_some() && r.value["outputs"].get("receipt").is_some()).count();
            let state=json!({"provider_status":raw["status"],"local_status":local[0]["status"],"applied_payments":payment_rows.len(),"receipt_claims":receipts.len(),"schedule_sequence":sequence,"completed_scheduled_runs":completed.saturating_sub(completed_before)});
            if state!=last {eprintln!("Bank settlement observation: {state}");last=state.clone();}
            save(&root.join("bank-auto-observation.json"),&json!({"state":state,"provider":raw,"payments":payments,"notices":notices,"payment_status_overrides":false}))?;
            if raw["status"]!="Paid" {
                ensure!(receipts.is_empty(),"Receipt was claimed before bank settlement");
                ensure!(notices==before,"Pending bank payment repeated a notification");
            } else {
                ensure!(payment_rows.len()==1&&payment_rows[0]["type"]=="transaction"&&payment_rows[0]["transaction"].is_object()&&payment_rows[0]["amount"].as_f64()==Some(1.0)&&payment_rows[0]["currencyCode"]=="USD","Require one actual USD 1 bank transaction; no manual settlement substitutes");
                if local[0]["status"]=="paid"&&receipts.len()==1&&receipts[0]["status"].as_str().is_some_and(|s|s.starts_with("accepted:")) {
                    let receipt_id=receipts[0]["status"].as_str().unwrap().to_owned();
                    let original=first_receipt_id.get_or_insert(receipt_id.clone());
                    ensure!(*original==receipt_id,"Receipt delivery ID changed during scheduled replay");
                    let first=*first_receipt_sequence.get_or_insert(sequence);
                    let first_done=*first_receipt_completed.get_or_insert(completed);
                    if sequence>=first+2 && completed>=first_done+2 {
                        save(&root.join("bank-auto-report.json"),&json!({"passed":true,"invoice":invoice,"automatic_scheduled_reconciliation":true,"bank_payment_records":1,"receipt_claims":1,"post_receipt_occurrences":sequence-first,"manual_status_overrides":false,"provider":raw,"payments":payments,"notices":notices}))?;
                        return Ok(());
                    }
                }
            }
            if tokio::time::Instant::now()>=deadline {
                save(&root.join("bank-auto-report.json"),&json!({"passed":false,"reason":if raw["status"]!="Paid" {"provider_settlement_pending"} else {"automatic_reconciliation_receipt_or_replay_not_complete"},"observation":state,"scheduled_occurrences":sequence-prior,"automatic_pending_checks_completed":completed.saturating_sub(completed_before)>=2,"manual_status_overrides":false,"checkpoint_retained":true}))?;
                anyhow::bail!("Actual bank settlement/receipt not yet verified: {state}; checkpoint retained, no status override or repeated payment");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }.await;
    outcome
}

/// Quiesce dog-queue before this control write. A concurrent recurrence can
/// otherwise change the schedule revision during teardown. Never retry commit;
/// reconcile the read-back state if its acknowledgement was lost.
pub(super) async fn pause(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    store: &Store,
    root: &Path,
) -> Result<()> {
    let pointer = root.join("bank-auto-latest.json");
    if !pointer.exists() {
        return Ok(());
    }
    let latest: serde_json::Value = serde_json::from_slice(&std::fs::read(pointer)?)?;
    let id = latest["schedule"]
        .as_str()
        .context("Missing campaign schedule")?;
    server::automation::model::identifier(id)?;
    let directory = std::path::PathBuf::from(
        latest["directory"]
            .as_str()
            .context("Missing evidence directory")?,
    );
    ensure!(
        directory.parent() == Some(root),
        "Evidence directory must belong to this fixture"
    );
    let Some(record) = store.get("schedule", id).await? else {
        return save(
            &directory.join("bank-auto-schedule.json"),
            &json!({"id":id,"never_started":true,"enabled":false}),
        );
    };
    let mut schedule = record.value;
    schedule["enabled"] = json!(false);
    let request = ok(
        app,
        tenant,
        token,
        "/automation",
        json!({"action":"schedule","schedule":schedule}),
        None,
    )
    .await;
    let current = store
        .get("schedule", id)
        .await?
        .context("Schedule disappeared during pause")?;
    let disabled = current.status == "disabled" && current.value["enabled"] == false;
    save(
        &directory.join("bank-auto-schedule.json"),
        &json!({"id":id,"enabled":current.value["enabled"],"pause_confirmed":disabled,"request_acknowledged":request.is_ok(),"sequence_retained":current.value["sequence"],"new_bounded_campaign_on_retry":true}),
    )?;
    if disabled {
        return Ok(());
    }
    request?;
    anyhow::bail!("Schedule pause not confirmed; retain the checkpoint for review")
}
