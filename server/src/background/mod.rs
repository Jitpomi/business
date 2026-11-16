//! dog-queue owns execution, leases, retries and graceful worker draining.
//! This supervisor only discovers tenant namespaces and backfills pre-cutover outboxes.
pub mod backend;
pub(crate) mod jobs;
use crate::automation::{
    engine::Engine,
    store::{q, Store},
};
use anyhow::Result;
use dog_queue::{Job, QueueAdapter, QueueConfig, QueueCtx, WorkerHandle};
use std::{collections::BTreeMap, time::Duration};
#[must_use = "Await shutdown before retiring a database"]
pub struct Worker {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}
impl Worker {
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
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
            match crate::storage::write(&store.state, &query).await {
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
pub fn spawn_all(template: Engine) -> Worker {
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut pools: BTreeMap<String, WorkerHandle> = BTreeMap::new();
        let mut cursor = String::new();
        let mut clock = tokio::time::interval(Duration::from_secs(5));
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let supervision: Result<()> = async {
        loop {
            tokio::select! {biased;_=&mut stopped=>break,_=clock.tick()=>{}}
            let after = if cursor.is_empty() {
                String::new()
            } else {
                format!("$tenant > {};", q(&cursor))
            };
            let query=format!("match $r isa automation_record,has auto_tenant $tenant;{after}select $tenant;distinct;sort $tenant;limit 100;fetch {{'tenant':$tenant}};");
            let rows = match crate::storage::read(&template.store.state, &query).await {
                Ok(rows) => rows,
                Err(_) => {
                    tracing::warn!("Background tenant discovery failed");
                    continue;
                }
            };
            if rows.is_empty() {
                cursor.clear();
                continue;
            }
            for row in rows {
                let Some(tenant) = row["tenant"].as_str() else {
                    continue;
                };
                cursor = tenant.into();
                if pools.get(tenant).is_some_and(|pool| !pool.is_finished()) { continue; }
                if let Some(pool) = pools.remove(tenant) {
                    if pool.shutdown().await.is_err() { tracing::error!(tenant,"Background pool failed; recovering durable deliveries"); }
                }
                let store = Store::new(template.store.state.clone(), tenant)?;
                if reconcile(&store).await.is_err() {
                    tracing::warn!(tenant, "Background outbox recovery deferred");
                    continue;
                }
                let config = QueueConfig {
                    max_workers: 4,
                    poll_interval: Duration::from_secs(2),
                    worker_idle_timeout: Duration::MAX,
                    lease_duration: Duration::from_secs(300),
                    heartbeat_interval: Duration::from_secs(60),
                    ..Default::default()
                };
                let backend = backend::TypeDBBackend {
                    store: store.clone(),
                    lease_duration: config.lease_duration,
                };
                let queue = QueueAdapter::try_with_config(backend, config)?;
                queue.register_job::<jobs::Advance>().await?;
                let engine = Engine {
                    store,
                    catalog: template.catalog.clone(),
                    vault: template.vault.clone(),
                };
                pools.insert(
                    tenant.into(),
                    queue
                        .start_workers(
                            QueueCtx::new(tenant),
                            jobs::Context {engine, capacity:capacity.clone()},
                            vec![jobs::Advance::JOB_TYPE.into()],
                        )
                        .await?,
                );
            }
        }
        Ok(())
        }.await;
        // Signal all pools together so another tenant cannot keep starting work
        // while an earlier pool is draining.
        let results =
            futures::future::join_all(pools.into_values().map(WorkerHandle::shutdown)).await;
        let errors: Vec<_> = results
            .into_iter()
            .filter_map(Result::err)
            .map(|e| e.to_string())
            .collect();
        supervision?;
        anyhow::ensure!(
            errors.is_empty(),
            "Background drain failed: {}",
            errors.join("; ")
        );
        Ok(())
    });
    Worker {
        stop: Some(stop),
        task: Some(task),
    }
}
#[cfg(test)]
#[path = "../../tests/unit/automation_worker.rs"]
mod worker_tests;
