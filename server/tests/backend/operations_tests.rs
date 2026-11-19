//! Real TypeDB operational aggregates and graph-complete recovery, without external effects.
use anyhow::{ensure, Result};
use std::sync::Arc;
use tower::ServiceExt;
#[tokio::test]
#[ignore = "Explicit development operator credentials, generated databases only; no provider or email calls"]
async fn operational_readiness_and_full_graph_recovery() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
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
    let source = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    let target = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    server::database::install(&driver, &source).await?;
    let out=async {
  let tx=driver.transaction(&source,typedb_driver::TransactionType::Write).await?;let t=uuid::Uuid::new_v4().to_string();let overdue=server::clock::unix_seconds()-1800;
  tx.query(format!("insert $t isa tenant,has tenant_id '{t}',has name 'Operational recovery fixture',has status 'active',has revision 'initial';$r isa resource,has resource_id '{}',has name 'Retained resource',has status 'active';(tenant:$t,resource:$r) isa resource_ownership;",uuid::Uuid::new_v4())).await?;
  for (kind,status,when) in [("job","failed",0),("job","enqueued",overdue)] {tx.query(format!("insert $r isa automation_record,has auto_key '{t}/{kind}/{status}',has auto_tenant '{t}',has auto_kind '{kind}',has auto_status '{status}',has auto_payload '{{}}',has auto_revision 'initial',has auto_when {when};")).await?;}tx.commit().await?;
  // Unknown sends and interrupted refunds must remain visible after the worker
  // has completed or restarted. Fresh work, provider-pending and settled history
  // must not be mistaken for a stalled submission.
  let tx=driver.transaction(&source,typedb_driver::TransactionType::Write).await?;
  let now=server::clock::unix_seconds();let at=server::clock::native_utc();
  tx.query(format!("match $t isa tenant,has tenant_id '{t}';insert $who isa person,has identity_id 'ops-owner',has name 'Owner',has status 'active';$c isa bill_customer,has bill_key 'ops-customer',has name 'Synthetic',has bill_email 'test@example.invalid',has bill_policy 'manual',has bill_choice 'manual',has bill_provider 'bank',has bill_bindings '{{}}',has bill_method_plugin '',has bill_payment_method '',has bill_consent '',has bill_revision 'initial';(merchant:$t,buyer:$t,customer:$c) isa bill_account;$p isa bill_plan,has bill_key 'ops-plan',has name 'Plan',has bill_amount 100,has bill_due_days 0,has bill_anchor {now},has bill_next {now},has bill_sequence 0,has bill_interval 'once',has bill_status 'active';(customer:$c,plan:$p) isa bill_plan_owner;$i isa bill_invoice,has bill_key 'ops-invoice',has name 'Invoice',has bill_amount 100,has bill_anchor {now},has bill_next {now},has bill_sequence 0,has bill_choice 'manual',has bill_provider 'bank',has bill_status 'paid',has bill_external_id 'provider-invoice',has bill_url '',has bill_updated {now},has bill_revision 'initial';(plan:$p,invoice:$i) isa bill_invoice_owner;")).await?;
  for (id,status,updated) in [("unknown","attention",now),("interrupted","sending",overdue),("fresh","sending",now),("delivered","accepted:original",overdue)] {
   tx.query(format!("match $i isa bill_invoice,has bill_key 'ops-invoice';insert $n isa bill_notice,has bill_key '{id}',has bill_status '{status}',has bill_updated {updated};(invoice:$i,notice:$n) isa bill_notice_owner;")).await?;
  }
  for (id,status,old) in [("queued-old","queued",true),("processing-old","processing",true),("processing-fresh","processing",false),("provider-pending","pending",true),("settled","succeeded",true)] {
   let created=if old{format!("{at} - PT30M")}else{at.clone()};
   tx.query(format!("match $i isa bill_invoice,has bill_key 'ops-invoice';$who isa person,has identity_id 'ops-owner';let $created={created};insert $r isa commerce_refund,has cx_id '{id}',has cx_amount 1,has cx_created == $created,has cx_refund_status '{status}';(invoice:$i,request:$r,actor:$who) isa commerce_refund_owner;")).await?;
  }
  tx.commit().await?;
  let state=Arc::new(server::typedb::TypeDBState{driver:driver.clone(),database:source.clone()});let values=server::operations::metrics(&state).await?;ensure!(values["failed_jobs"]==1&&values["overdue_jobs"]==1&&values["attention_runs"]==0,"Wrong operational aggregate: {values}");
  ensure!(values["attention_notices"]==2&&values["stalled_refunds"]==2,"Uncertain effects are invisible or fresh/pending history was miscounted: {values}");
  let dir=tempfile::tempdir()?;server::database::snapshot::checkpoint(&driver,&source,dir.path()).await?;let original=server::database::snapshot::graph(&driver,&source,&[]).await?;
  ensure!(server::recovery::restore(&driver,dir.path(),&source,&source).await.is_err());ensure!(server::recovery::restore(&driver,dir.path(),&source,"business_dev").await.is_err());
  server::recovery::restore(&driver,dir.path(),&source,&target).await?;ensure!(server::database::snapshot::graph(&driver,&source,&[]).await?==original);ensure!(server::database::snapshot::graph(&driver,&target,&[]).await?==original);ensure!(server::recovery::restore(&driver,dir.path(),&source,&target).await.is_err());
  // The online checkpoint is manifested from its own transactionally exported graph.
  let online=tempfile::tempdir()?;server::database::snapshot::online_checkpoint(&driver,&driver,&source,online.path()).await?;
  std::env::set_var("BUSINESS_WRITERS_STOPPED","true");
  server::operations::refresh_control_policy(&driver,&source,&dir.path().join(format!("{source}.graph.json"))).await?;
  std::env::remove_var("BUSINESS_WRITERS_STOPPED");
  let tx=driver.transaction(&source,typedb_driver::TransactionType::Write).await?;tx.query(format!("match $t isa tenant,has tenant_id '{t}';insert $r isa resource,has resource_id '{}',has name 'Written after checkpoint',has status 'active';(tenant:$t,resource:$r) isa resource_ownership;",uuid::Uuid::new_v4())).await?;tx.commit().await?;
  let newer=server::database::snapshot::graph(&driver,&source,&[]).await?;ensure!(newer!=original);
  let proof=server::recovery::rehearse(&driver,online.path(),&source).await?;ensure!(proof["graph_verified"]==true&&proof["generated_shadow_removed"]==true);ensure!(server::database::snapshot::graph(&driver,&source,&[]).await?==newer,"Rehearsal rolled back source writes");
  // The fence runs before transport dispatch. A forged custom write cannot reach a service.
  let app=axum::Router::new().fallback(||async{axum::http::StatusCode::OK}).layer(axum::middleware::from_fn(server::operations::recovery_guard));
  let r=axum::http::Request::builder().method("GET").uri("/data").header("x-service-method","write").body(axum::body::Body::empty())?;ensure!(app.oneshot(r).await?.status()==503);
  Ok::<_,anyhow::Error>(())
 }.await;
    if driver.databases().contains(&target).await? {
        driver.databases().get(&target).await?.delete().await?;
    }
    driver.databases().get(&source).await?.delete().await?;
    out
}
