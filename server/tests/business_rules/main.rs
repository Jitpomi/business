//! Adversarial policy tests execute native functions and writes without service preflights.
#[path = "../support/driver.rs"]
mod test_driver;
use anyhow::{ensure, Result};
use serde_json::json;
use server::{
    automation::{
        connectors::Catalog,
        engine::Engine,
        model::{Attempt, Effect, Run, Step, Workflow},
        store::Store,
        vault::Vault,
    },
    database,
    typedb::TypeDBState,
};
use std::sync::Arc;
#[path = "../support/mod.rs"]
mod support;
#[tokio::test]
#[ignore = "Real development TypeDB; generated database only; no provider calls"]
async fn native_rules_reject_bypassed_checks_and_revoked_writers() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    let driver = Arc::new(
        crate::test_driver::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    database::install(&driver, &db).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: db.clone(),
    });
    let result=async {
        let tenant=uuid::Uuid::new_v4().to_string();let actor=uuid::Uuid::new_v4().to_string();
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        tx.query(format!(r#"insert $t isa tenant,has tenant_id "{tenant}",has status "active",has name "Policy test",has revision "initial";
$u isa person,has identity_id "{actor}",has email "native-policy@example.invalid",has name "Owner",has status "active";
(tenant:$t,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;
$c isa bill_customer,has bill_key "customer",has name "Customer",has bill_email "customer@example.invalid",has bill_policy "customer",has bill_choice "manual",has bill_provider "test",has bill_bindings "{{}}",has bill_method_plugin "",has bill_payment_method "",has bill_consent "",has bill_revision "initial";
(merchant:$t,buyer:$t,customer:$c) isa bill_account;
$p isa bill_plan,has bill_key "plan",has name "Plan",has bill_amount 100,has bill_due_days 0,has bill_anchor 0,has bill_next 0,has bill_sequence 0,has bill_interval "month",has bill_status "active";
(customer:$c,plan:$p) isa bill_plan_owner;
$i isa bill_invoice,has bill_key "invoice",has name "Invoice",has bill_amount 100,has bill_anchor 0,has bill_next 0,has bill_sequence 0,has bill_choice "manual",has bill_provider "test",has bill_status "open",has bill_external_id "external",has bill_url "",has bill_updated 0,has bill_revision "initial";
(plan:$p,invoice:$i) isa bill_invoice_owner;"#)).await?;tx.commit().await?;
        for (status,due,now,days,expected) in [
            ("paid",0,100,7,Some(("receipt",0))), ("processing",0,100,7,None),
            ("open",100,99,7,Some(("invoice",0))), ("open",100,100,7,Some(("reminder",0))),
            ("open",0,86400*21,7,Some(("reminder",3))), ("open",0,86400*29,7,None),
            ("open",0,100,0,None), ("paid",0,100,31,None)
        ] {
            storage::write(&state,&format!("match $t isa tenant;$i isa bill_invoice,has bill_key 'invoice';select $t,$i;update $i has bill_status '{status}',has bill_next {due};fetch {{'id':$i.bill_key}};")).await?;
            let rows=storage::read(&state,&format!("match $i isa bill_invoice,has bill_key 'invoice';let $kind,$bucket=bill_notice_decision($i,{now},{days});fetch {{'kind':$kind,'bucket':$bucket}};")).await?;
            if let Some((kind,bucket))=expected {ensure!(rows.len()==1&&rows[0]["kind"]==kind&&server::billing::engine::number(&rows[0],"bucket")?==bucket,"Native notice decision differed");} else {ensure!(rows.is_empty(),"Native policy allowed an ineligible notice");}
        }
        storage::write(&state,"match $t isa tenant;$i isa bill_invoice;select $t,$i;update $i has bill_status 'paid';fetch {'id':$i.bill_key};").await?;
        for (next,allowed) in [("open",false),("processing",false),("void",false),("paid",true)] {
            let rows=storage::read(&state,&format!("match $i isa bill_invoice;fetch {{'ok':bill_observation_transition($i,'{next}')}};")).await?;ensure!(rows[0]["ok"]==allowed,"Terminal invoice state was reversible");
        }
        let rows=storage::read(&state,"match $i isa bill_invoice;fetch {'valid':bill_observation_matches($i,'invoice','test','external',100,'USD'),'amount':bill_observation_matches($i,'invoice','test','external',99,'USD'),'currency':bill_observation_matches($i,'invoice','test','external',100,'EUR'),'binding':bill_observation_matches($i,'invoice','wrong','external',100,'USD')};").await?;
        ensure!(rows[0]["valid"]==true&&rows[0]["amount"]==false&&rows[0]["currency"]==false&&rows[0]["binding"]==false);
        storage::write(&state,"match $i isa bill_invoice; insert $n isa bill_notice,has bill_key 'history',has bill_status 'imported-history',has bill_updated 0;(invoice:$i,notice:$n) isa bill_notice_owner;fetch {'id':$n.bill_key};").await?;
        ensure!(storage::read(&state,"match $i isa bill_invoice;let $kind,$bucket=bill_notice_decision($i,100,7);fetch {'kind':$kind};").await?.is_empty(),"Imported history was eligible for a new receipt");
        let store=Store::new(state.clone(),&tenant)?;store.insert("settings","billing",&json!({"reminder_days":7}),"configured",0).await?;
        let scoped=store.for_owner(&actor)?;scoped.owner(&actor).await?;let old=scoped.get("settings","billing").await?.unwrap();
        storage::write(&state,&format!("match $t isa tenant;$u isa person,has identity_id '{actor}';$m isa membership,links(tenant:$t,member:$u);select $t,$m;update $m has membership_role 'member';fetch {{'id':$u.identity_id}};").replace("select $t,$m;","select $t,$m,$u;")).await?;
        ensure!(scoped.cas(&old,&json!({"reminder_days":8}),"configured",0).await.is_err(),"Revoked owner wrote after a successful preflight");
        ensure!(store.get("settings","billing").await?.unwrap().value==old.value);
        storage::write(&state,&format!("match $t isa tenant;$u isa person,has identity_id '{actor}';$m isa membership,links(tenant:$t,member:$u);select $t,$m,$u;update $m has membership_role 'owner';fetch {{'id':$u.identity_id}};")).await?;
        let engine=Engine{store:store.clone(),catalog:Arc::new(Catalog::default()),vault:Vault::default()};
        let run=Run{owner:actor.clone(),id:"approval".into(),workflow:Workflow{id:"flow".into(),revision:"version".into(),name:"Review".into(),enabled:true,steps:vec![Step::Approval{id:"review".into()}]},input:json!(null),outputs:Default::default(),cursor:0,status:"approval".into(),wake:0,attempt:None,approved:vec![],connections:Default::default(),last_error:None};
        let original=store.insert("run","approval",&serde_json::to_value(&run)?,"approval",0).await?;
        let mut forged=run.clone();forged.cursor=1;forged.status="completed".into();
        ensure!(store.cas(&original,&serde_json::to_value(&forged)?,"completed",0).await.is_err(),"Skipping native approval was allowed");
        engine.approve("approval",&actor).await?;ensure!(engine.advance("approval").await?["status"]=="completed");
        let finished=store.get("run","approval").await?.unwrap();forged.status="cancelled".into();
        ensure!(store.control(&finished,&serde_json::to_value(&forged)?,"cancelled",0,&actor,"cancel").await.is_err(),"Completed history was overwritten by cancellation");
        let mut uncertain=run;uncertain.id="uncertain".into();uncertain.workflow.steps=vec![Step::Action{id:"write".into(),connection:"crm".into(),action:"send".into(),input:json!(null)}];uncertain.status="attention".into();uncertain.attempt=Some(Attempt{key:"original".into(),count:1,effect:Effect::AtMostOnce,started:0});
        let mut inflight=uncertain.clone();inflight.id="inflight".into();inflight.status="running".into();
        let claimed=store.insert("run","inflight",&serde_json::to_value(&inflight)?,"running",0).await?;
        inflight.status="retry".into();inflight.attempt.as_mut().unwrap().effect=Effect::Idempotent;
        ensure!(store.cas(&claimed,&serde_json::to_value(&inflight)?,"retry",0).await.is_err(),"Changing the proposed effect bypassed the original at-most-once claim");
        let old=store.insert("run","uncertain",&serde_json::to_value(&uncertain)?,"attention",0).await?;
        let mut retry=uncertain;retry.status="retry".into();ensure!(store.cas(&old,&serde_json::to_value(&retry)?,"retry",0).await.is_err(),"Uncertain write was silently requeued");
        retry.cursor=1;retry.status="queued".into();retry.attempt=None;retry.outputs.insert("write".into(),json!({"evidence":"bad"}));
        ensure!(store.control(&old,&serde_json::to_value(&retry)?,"queued",0,&actor,"resolve").await.is_err(),"Resolution without sufficient evidence was accepted");
        ensure!(store.get("run","uncertain").await?.unwrap().value==old.value);
        Ok::<_,anyhow::Error>(())
    }.await;
    if result.is_ok() {
        driver.databases().get(&db).await?.delete().await?;
    } else {
        eprintln!("Retained native policy fixture: {db}");
    }
    result
}

// Raw operator transactions deliberately bypass every application preflight.
mod storage {
    use anyhow::{ensure, Result};
    use serde_json::Value;
    use server::typedb::TypeDBState;
    pub async fn read(state: &TypeDBState, query: &str) -> Result<Vec<Value>> {
        let tx = state
            .driver
            .transaction(&state.database, typedb_driver::TransactionType::Read)
            .await?;
        let rows = server::database::documents(&tx, query).await?;
        tx.close().await?;
        Ok(rows)
    }
    pub async fn write(state: &TypeDBState, query: &str) -> Result<Value> {
        let tx = state
            .driver
            .transaction(&state.database, typedb_driver::TransactionType::Write)
            .await?;
        let rows = server::database::documents(&tx, query).await?;
        ensure!(
            rows.len() == 1,
            "Raw fixture update did not match one record"
        );
        tx.commit().await?;
        Ok(rows.into_iter().next().unwrap())
    }
}

#[tokio::test]
#[ignore = "Restore-only migration rehearsal against an explicit immutable backup; generated shadows; no provider calls"]
async fn policy_shadow_preserves_all_backup_work_and_is_idempotent() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    support::auth()?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    let driver = crate::test_driver::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let backup = std::path::PathBuf::from(std::env::var("BUSINESS_RULES_TEST_BACKUP")?);
    let target = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Backup policy rehearsal shadow: {target}");
    let source = std::env::var("BUSINESS_RULES_TEST_SOURCE")
        .unwrap_or_else(|_| "business_native_dev_v3".into());
    let report = server::business_rules::import(&driver, &source, &target, &backup).await?;
    ensure!(
        report["preserved"] == true
            && report["idempotent"] == true
            && report["provider_calls"] == 0
    );
    eprintln!("Verified policy shadow: {report}");
    Ok(())
}
