//! dog-queue owns execution, leases, retries and graceful worker draining.
//! TypeDB adapters supply namespaces, schedules and durable outboxes; dog-queue owns supervision.
pub mod backend;
pub(crate) mod jobs;
pub mod recurrence;
use crate::automation::{
    engine::Engine,
    store::{q, Store},
};
use anyhow::Result;
use dog_queue::{Job, QueueAdapter, QueueConfig, QueueCtx, WorkerHandle};
use std::time::Duration;
pub type Worker = dog_queue::supervisor::SupervisorHandle;
/// Add only missing deliveries. Revision matching and absence are checked in the
/// same transaction, so this is safe against a writer or a second supervisor.
pub async fn reconcile(store: &Store) -> Result<usize> {
    let mut after = String::new();
    let mut inserted = 0;
    loop {
        let cursor = if after.is_empty() {
            String::new()
        } else {
            format!("$key > {};", q(&after))
        };
        let rows=crate::storage::read(&store.state,&format!("{} $r isa automation_record,has auto_tenant {},has auto_kind $kind,has auto_key $key,has auto_status $status,has auto_revision $rev,has auto_when $when;{{$kind == \"run\";{{$status == \"queued\";}} or {{$status == \"running\";}} or {{$status == \"retry\";}} or {{$status == \"waiting\";}};}} or {{$kind == \"schedule\";$status == \"scheduled\";}};{cursor}select $r,$key;sort $key;limit 100;fetch {{'key':$r.auto_key,'kind':$r.auto_kind,'revision':$r.auto_revision,'status':$r.auto_status,'when':$r.auto_when}};",store.prefix(),q(&store.tenant))).await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let key = row["key"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid workflow key"))?;
            after = key.into();
            let rev = row["revision"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid workflow revision"))?;
            if store.get("job", rev).await?.is_some() {
                continue;
            }
            let clause = jobs::outbox(
                store,
                row["kind"].as_str().unwrap_or(""),
                key.rsplit('/').next().unwrap_or(""),
                rev,
                row["status"].as_str().unwrap_or(""),
                row["when"].as_i64().unwrap_or(0),
            )?;
            anyhow::ensure!(!clause.is_empty(), "Invalid pending workflow");
            let query=format!("{} $r isa automation_record,has auto_tenant {},has auto_key {},has auto_revision {};not {{$existing isa automation_record,has auto_key {};}};select $t,$r;{clause}fetch {{'id':$job.auto_key}};",store.prefix(),q(&store.tenant),q(key),q(rev),q(&store.key("job",rev)));
            match crate::storage::write_queue(&store.state, &query).await {
                Ok(_) => inserted += 1,
                Err(e) => {
                    // Reconcile a raced or uncertain commit by exact immutable ID.
                    if store.get("job", rev).await?.is_none()
                        && store
                            .get(
                                row["kind"].as_str().unwrap_or(""),
                                key.rsplit('/').next().unwrap_or(""),
                            )
                            .await?
                            .is_some_and(|r| r.revision == rev)
                    {
                        return Err(e);
                    }
                }
            }
        }
    }
    Ok(inserted)
}
struct TenantSource {
    template: Engine,
    capacity: std::sync::Arc<tokio::sync::Semaphore>,
}
#[async_trait::async_trait]
impl dog_queue::supervisor::TenantWorkers for TenantSource {
    async fn page(&self, after: Option<&str>) -> dog_queue::QueueResult<Vec<String>> {
        let after = after
            .map(|cursor| format!("$tenant > {};", q(cursor)))
            .unwrap_or_default();
        let query=format!("match $r isa automation_record,has auto_tenant $tenant;{after}select $tenant;distinct;sort $tenant;limit 100;fetch {{'tenant':$tenant}};");
        let rows = crate::storage::read(&self.template.store.state, &query)
            .await
            .map_err(|_| {
                dog_queue::QueueError::Internal("TypeDB tenant discovery unavailable".into())
            })?;
        rows.into_iter()
            .map(|row| {
                row["tenant"].as_str().map(str::to_owned).ok_or_else(|| {
                    dog_queue::QueueError::Internal("Invalid tenant discovery row".into())
                })
            })
            .collect()
    }
    async fn start(&self, tenant: &str) -> dog_queue::QueueResult<WorkerHandle> {
        let store = Store::new(self.template.store.state.clone(), tenant)
            .map_err(|_| dog_queue::QueueError::InvalidConfig("Invalid tenant namespace".into()))?;
        reconcile(&store).await.map_err(|_| {
            dog_queue::QueueError::Internal("Durable outbox recovery deferred".into())
        })?;
        let config = QueueConfig {
            max_workers: 4,
            poll_interval: Duration::from_secs(2),
            worker_idle_timeout: Duration::MAX,
            lease_duration: Duration::from_secs(300),
            heartbeat_interval: Duration::from_secs(60),
            ..Default::default()
        };
        let queue = QueueAdapter::try_with_config(
            backend::TypeDBBackend {
                store: store.clone(),
                lease_duration: config.lease_duration,
            },
            config,
        )?;
        queue.register_job::<jobs::Advance>().await?;
        queue
            .register_job::<dog_queue::recurrence::Scheduled<recurrence::TriggerWorkflow>>()
            .await?;
        let engine = Engine {
            store,
            catalog: self.template.catalog.clone(),
            vault: self.template.vault.clone(),
        };
        queue
            .start_workers(
                QueueCtx::new(tenant),
                jobs::Context {
                    engine,
                    capacity: self.capacity.clone(),
                },
                vec![
                    jobs::Advance::JOB_TYPE.into(),
                    <recurrence::TriggerWorkflow as dog_queue::recurrence::ScheduledJob>::JOB_TYPE
                        .into(),
                ],
            )
            .await
    }
}
pub fn spawn_all(template: Engine) -> Worker {
    dog_queue::supervisor::start(std::sync::Arc::new(TenantSource {
        template,
        capacity: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
    }))
}
