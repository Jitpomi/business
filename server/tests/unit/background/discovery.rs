use super::*;
use anyhow::{ensure, Result};
use dog_queue::{JobMessage, QueueBackend, QueueCtx};
use serde_json::json;
use std::sync::Arc;

#[test]
fn discovery_query_is_a_single_read_pipeline() {
    for after in [None, Some("tenant-with-a-quote-\"")] {
        let query = discovery_query(after, 1_700_000_000);
        let parsed = typeql::parse_queries(&query).unwrap();
        assert_eq!(parsed.len(), 1);
    }
}

#[tokio::test]
#[ignore = "Loopback TypeDB only; isolated durable queue fixtures, no provider effects"]
async fn due_discovery_preserves_history_future_jobs_and_missing_outboxes() -> Result<()> {
    let driver = Arc::new(crate::recovery::local_driver().await?);
    let database = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    crate::database::install(&driver, &database).await?;
    let tenant = uuid::Uuid::new_v4().to_string();
    let owner = uuid::Uuid::new_v4().to_string();
    let tx = driver
        .transaction(&database, typedb_driver::TransactionType::Write)
        .await?;
    tx.query(format!("insert $t isa tenant,has tenant_id {},has name 'Queue discovery test',has status 'active',has revision 'initial';$i isa person,has identity_id {},has name 'Synthetic queue owner',has status 'active';(tenant:$t,member:$i) isa membership,has membership_role 'owner',has start_at 2025-01-01T00:00:00;",q(&tenant),q(&owner))).await?;
    tx.commit().await?;
    let state = Arc::new(crate::typedb::TypeDBState {
        driver: driver.clone(),
        database,
    });
    let store = Store::new(state.clone(), &tenant)?;
    let engine = Engine {
        store: store.clone().for_owner(&owner)?,
        catalog: Default::default(),
        vault: Default::default(),
    };
    let due = |at| {
        let state = &state;
        async move { crate::storage::read(state, &discovery_query(None, at)).await }
    };
    engine
        .publish(
            crate::automation::model::Workflow {
                id: "discovery".into(),
                revision: String::new(),
                name: "Synthetic local transformation".into(),
                enabled: true,
                steps: vec![crate::automation::model::Step::Transform {
                    id: "value".into(),
                    value: json!(true),
                }],
            },
            &owner,
        )
        .await?;
    ensure!(
        due(chrono::Utc::now().timestamp()).await?.is_empty(),
        "Workflow history started an idle pool"
    );
    let result = engine
        .enqueue("discovery", "original-request", json!({}))
        .await?;
    let run = result["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing run"))?;
    let original = store.get("run", run).await?.unwrap();
    let job_key = store.key("job", &original.revision);
    // Simulate an older imported outbox gap in this isolated namespace only.
    let tx = driver
        .transaction(&state.database, typedb_driver::TransactionType::Write)
        .await?;
    tx.query(format!(
        "match $r isa automation_record,has auto_tenant {},has auto_key {};delete $r;",
        q(&tenant),
        q(&job_key)
    ))
    .await?;
    tx.commit().await?;
    let rows = due(chrono::Utc::now().timestamp()).await?;
    ensure!(
        rows.len() == 1 && rows[0]["tenant"] == tenant,
        "Due run with a missing delivery was hidden"
    );
    ensure!(
        reconcile(&store).await? == 1
            && store.get("run", run).await?.unwrap().value == original.value,
        "Recovery changed the workflow or lost its outbox"
    );
    ensure!(
        crate::storage::read(
            &state,
            &discovery_query(Some(&tenant), chrono::Utc::now().timestamp())
        )
        .await?
        .is_empty(),
        "Discovery cursor repeated its tenant"
    );
    let discovered = due(chrono::Utc::now().timestamp()).await?;
    ensure!(
        discovered.len() == 1,
        "Run and delivery duplicated their tenant: {discovered:?}"
    );
    let worker = spawn_all(engine);
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            if store
                .get("run", run)
                .await?
                .is_some_and(|r| r.status == "completed")
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    worker.shutdown().await?;
    ensure!(
        !store.list("job", 100).await?.is_empty(),
        "Completed delivery history disappeared"
    );
    ensure!(
        due(chrono::Utc::now().timestamp()).await?.is_empty(),
        "Completed history kept an idle namespace eligible"
    );
    let backend = backend::TypeDBBackend {
        store: store.clone(),
        lease_duration: Duration::from_secs(30),
    };
    let ctx = QueueCtx::new(&tenant);
    let future_at = chrono::Utc::now() + chrono::Duration::hours(1);
    let future = backend
        .enqueue(
            ctx.clone(),
            JobMessage::new("contract", b"{}".to_vec(), "json", "contract").with_run_at(future_at),
        )
        .await?;
    let before = backend.get_record(ctx.clone(), future.clone()).await?;
    ensure!(
        due(chrono::Utc::now().timestamp()).await?.is_empty(),
        "Future work started an early pool"
    );
    ensure!(
        due(future_at.timestamp() + 1).await?.len() == 1,
        "Future delivery could not become discoverable"
    );
    ensure!(
        backend.dequeue(ctx.clone(), &["contract"]).await?.is_none(),
        "Future work executed early"
    );
    ensure!(
        serde_json::to_value(backend.get_record(ctx, future).await?)?
            == serde_json::to_value(before)?,
        "Discovery rewrote future work"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "Read-only loopback performance rehearsal against an explicitly selected retained checkpoint"]
async fn retained_checkpoint_discovers_only_due_work() -> Result<()> {
    let database = std::env::var("BUSINESS_QUEUE_CHECKPOINT_DATABASE")?;
    ensure!(
        database
            .strip_prefix("business_backend_test_")
            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok()),
        "Generated local checkpoint required"
    );
    let driver = Arc::new(crate::recovery::local_driver().await?);
    let state = crate::typedb::TypeDBState { driver, database };
    let start = std::time::Instant::now();
    let all=crate::storage::read(&state,"match $r isa automation_record,has auto_tenant $tenant;select $tenant;distinct;sort $tenant;limit 100;fetch {'tenant':$tenant};").await?;
    let old_ms = start.elapsed().as_millis();
    let start = std::time::Instant::now();
    let due = crate::storage::read(
        &state,
        &discovery_query(None, chrono::Utc::now().timestamp()),
    )
    .await?;
    let new_ms = start.elapsed().as_millis();
    ensure!(
        due.iter().all(|row| all.contains(row)),
        "Discovery invented a tenant"
    );
    let completed=crate::storage::read(&state,"match $r isa automation_record,has auto_kind 'job',has auto_status 'completed';reduce $n=count;fetch {'count':$n};").await?;
    println!(
        "{}",
        json!({"all_historical_namespaces":all.len(),"due_namespaces":due.len(),"completed_deliveries_retained":completed[0]["count"],"old_discovery_ms":old_ms,"due_discovery_ms":new_ms,"read_only":true})
    );
    Ok(())
}
