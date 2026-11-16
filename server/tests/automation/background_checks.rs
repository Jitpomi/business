//! Real TypeDB queue contract, including competing consumers and lost-worker recovery.
use anyhow::{ensure, Result};
use dog_queue::{JobId, JobMessage, JobStatus, LeaseToken, QueueBackend, QueueCtx};
use server::{automation::store::Store, background::backend::TypeDBBackend};
use std::time::Duration;
pub async fn exercise(store: &Store, other: &Store) -> Result<()> {
    let backend = TypeDBBackend {
        store: store.clone(),
        lease_duration: Duration::from_secs(3),
    };
    let second = TypeDBBackend {
        store: store.clone(),
        lease_duration: Duration::from_secs(3),
    };
    let ctx = QueueCtx::new(&store.tenant);
    let foreign = QueueCtx::new(&other.tenant);
    let message = JobMessage::new("contract", b"{}".to_vec(), "json", "contract")
        .with_idempotency_key("same");
    let id = backend.enqueue(ctx.clone(), message.clone()).await?;
    ensure!(backend.enqueue(ctx.clone(), message.clone()).await? == id);
    ensure!(backend.enqueue(foreign.clone(), message).await.is_err());
    let outsider = TypeDBBackend {
        store: other.clone(),
        lease_duration: Duration::from_secs(3),
    };
    ensure!(outsider.get_record(foreign, id.clone()).await.is_err());
    let (a, b) = tokio::join!(
        backend.dequeue(ctx.clone(), &["contract"]),
        second.dequeue(ctx.clone(), &["contract"])
    );
    let leases = [a, b]
        .into_iter()
        .filter_map(|r| r.ok().flatten())
        .collect::<Vec<_>>();
    ensure!(
        leases.len() == 1,
        "Competing consumers both acquired a lease"
    );
    let first = &leases[0];
    ensure!(backend
        .ack_complete(ctx.clone(), id.clone(), LeaseToken::new(), None)
        .await
        .is_err());
    tokio::time::sleep(Duration::from_millis(3100)).await;
    ensure!(backend
        .ack_complete(ctx.clone(), id.clone(), first.lease_token.clone(), None)
        .await
        .is_err());
    ensure!(backend.dequeue(ctx.clone(), &["contract"]).await?.is_none()); // durable expiry -> retry
    let recovered = backend
        .dequeue(ctx.clone(), &["contract"])
        .await?
        .expect("expired delivery disappeared");
    ensure!(recovered.record.attempt == 2);
    ensure!(backend
        .ack_complete(ctx.clone(), id.clone(), first.lease_token.clone(), None)
        .await
        .is_err());
    ensure!(backend.cancel(ctx.clone(), id.clone()).await?);
    ensure!(backend
        .ack_complete(ctx.clone(), id.clone(), recovered.lease_token, None)
        .await
        .is_err());
    ensure!(matches!(
        backend.get_status(ctx.clone(), id).await?,
        JobStatus::Canceled { .. }
    ));
    let future = backend
        .enqueue(
            ctx.clone(),
            JobMessage::new("contract", b"{}".to_vec(), "json", "contract")
                .with_run_at(chrono::Utc::now() + chrono::Duration::hours(1)),
        )
        .await?;
    ensure!(
        backend.dequeue(ctx.clone(), &["contract"]).await?.is_none(),
        "Delayed delivery ran early"
    );
    backend.cancel(ctx.clone(), future).await?;
    // Hold a business fence update open while queue ownership changes commit.
    // Queue metadata must not invalidate a payment's independent transaction.
    let business = store
        .state
        .driver
        .transaction(&store.state.database, typedb_driver::TransactionType::Write)
        .await?;
    business
        .query(format!(
            "match $t isa tenant,has tenant_id {};update $t has revision {};",
            serde_json::to_string(&store.tenant)?,
            serde_json::to_string(&uuid::Uuid::new_v4().to_string())?
        ))
        .await?;
    let done = backend
        .enqueue(
            ctx.clone(),
            JobMessage::new("contract", b"{}".to_vec(), "json", "contract"),
        )
        .await?;
    let job = backend.dequeue(ctx.clone(), &["contract"]).await?.unwrap();
    backend
        .heartbeat_extend(
            ctx.clone(),
            done.clone(),
            job.lease_token.clone(),
            Duration::from_secs(60),
        )
        .await?;
    backend
        .ack_complete(
            ctx.clone(),
            done.clone(),
            job.lease_token,
            Some("null".into()),
        )
        .await?;
    ensure!(matches!(
        backend.get_status(ctx.clone(), done).await?,
        JobStatus::Completed { .. }
    ));
    business.commit().await?; // A shared tenant-wide queue fence would conflict here.
    ensure!(backend.get_record(ctx, JobId::new()).await.is_err());
    Ok(())
}
