//! Typed deliveries reference immutable workflow revisions, never provider credentials.
use crate::automation::{engine::Engine, store::Store};
use async_trait::async_trait;
use dog_queue::{Job, JobError};
use serde::{Deserialize, Serialize};
#[derive(Clone)]
pub struct Context {
    pub engine: Engine,
    pub capacity: std::sync::Arc<tokio::sync::Semaphore>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Advance {
    pub kind: String,
    pub id: String,
    pub revision: String,
}
#[async_trait]
impl Job for Advance {
    type Context = Context;
    type Result = ();
    const JOB_TYPE: &'static str = "business.advance";
    const MAX_RETRIES: u32 = 3;
    async fn execute(&self, context: Context) -> Result<(), JobError> {
        let _permit = context
            .capacity
            .acquire()
            .await
            .map_err(|_| JobError::permanent("Background capacity closed"))?;
        let engine = context.engine;
        let result = async {
            let Some(record) = engine.store.get(&self.kind, &self.id).await? else {
                return Ok(());
            };
            // Another delivery or a human may already have advanced/cancelled it.
            if record.revision != self.revision {
                return Ok(());
            }
            match self.kind.as_str() {
                "run" => {
                    engine.advance(&self.id).await?;
                }
                "schedule" => {
                    dog_queue::recurrence::dispatch(
                        &super::recurrence::TypeDBRecurrence {
                            store: engine.store.clone(),
                        },
                        &dog_queue::QueueCtx::new(&engine.store.tenant),
                        &self.id,
                        &self.revision,
                        chrono::Utc::now(),
                    )
                    .await?;
                }
                _ => anyhow::bail!("Invalid workflow delivery kind"),
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        result.map_err(|_| {
            JobError::retryable("Workflow delivery deferred; durable journal retained")
        })
    }
}
/// Called inside the same TypeDB transaction that changes the workflow record.
/// This is a transactional outbox: no dual-write window or separate scheduler.
pub(crate) fn outbox(
    store: &Store,
    kind: &str,
    id: &str,
    revision: &str,
    status: &str,
    when: i64,
) -> anyhow::Result<String> {
    if !eligible(kind, status) {
        return Ok(String::new());
    }
    let delivery = Advance {
        kind: kind.into(),
        id: id.into(),
        revision: revision.into(),
    };
    let message = dog_queue::CodecRegistry::new().encode_job(
        &delivery,
        dog_queue::EnqueueOptions::scheduled(
            chrono::DateTime::from_timestamp(when, 0)
                .ok_or_else(|| anyhow::anyhow!("Invalid delivery time"))?,
        ),
    )?;
    let record = super::backend::Persisted::new(&store.tenant, message, Some(revision));
    super::backend::insert_clause(store, &record)
}
pub(crate) fn eligible(kind: &str, status: &str) -> bool {
    (kind == "run" && ["queued", "running", "retry", "waiting"].contains(&status))
        || (kind == "schedule" && status == "scheduled")
}
