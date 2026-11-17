//! TypeDB storage adapter for dog-queue recurrence. No recurrence arithmetic lives here.
use super::{
    backend::{insert_clause_named, Persisted},
    jobs::{outbox, Context},
};
use crate::automation::{
    model::Schedule,
    store::{q, Store},
};
use async_trait::async_trait;
use dog_queue::{
    recurrence::{
        IntervalCursor, Occurrence, RecurrenceBackend, ScheduleRecord, ScheduledJob, Template,
    },
    JobError, JobMessage, QueueCtx, QueueError, QueueResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone)]
pub struct TypeDBRecurrence {
    pub store: Store,
}
fn error(_: anyhow::Error) -> QueueError {
    QueueError::Internal("TypeDB schedule transaction failed; durable cursor retained".into())
}
#[derive(Clone, Serialize, Deserialize)]
pub struct TriggerWorkflow {
    pub workflow: String,
    pub input: Value,
}
#[async_trait]
impl ScheduledJob for TriggerWorkflow {
    type Context = Context;
    type Result = ();
    const JOB_TYPE: &'static str = "business.scheduled-workflow";
    async fn execute(&self, context: Context, occurrence: Occurrence) -> Result<(), JobError> {
        let _permit = context
            .capacity
            .acquire()
            .await
            .map_err(|_| JobError::permanent("Background capacity closed"))?;
        let mut engine = context.engine;
        let old = engine
            .store
            .get("schedule", &occurrence.schedule_id)
            .await
            .map_err(|_| JobError::retryable("Schedule unavailable"))?
            .ok_or_else(|| JobError::permanent("Schedule unavailable"))?;
        let schedule: Schedule = serde_json::from_value(old.value.clone())
            .map_err(|_| JobError::permanent("Invalid schedule"))?;
        if !schedule.enabled || old.status != "scheduled" {
            return Err(JobError::retryable("Schedule paused; occurrence retained"));
        }
        if schedule.workflow != self.workflow || schedule.input != self.input {
            return Err(JobError::permanent(
                "Schedule definition changed; occurrence retained",
            ));
        }
        engine.store = engine
            .store
            .for_origin(&old)
            .map_err(|_| JobError::permanent("Invalid schedule origin"))?;
        // This is business dispatch, not recurrence. Native owner/connection/consent
        // checks remain in the existing TypeDB business paths and payment journal.
        match engine
            .enqueue(&self.workflow, &occurrence.key(), self.input.clone())
            .await
        {
            Ok(_) => Ok(()),
            Err(cause) => {
                // Only a definitive native denial pauses the business schedule. A database
                // or provider-store outage must not be misclassified as revoked permission.
                if cause
                    .downcast_ref::<dog_core::DogError>()
                    .is_some_and(|e| e.kind == dog_core::ErrorKind::Forbidden)
                {
                    engine
                        .store
                        .cas(&old, &old.value, "blocked", 0)
                        .await
                        .map_err(|_| {
                            JobError::retryable("Schedule review could not be recorded")
                        })?;
                }
                Err(JobError::retryable(
                    "Workflow dispatch deferred; occurrence retained",
                ))
            }
        }
    }
}
#[async_trait]
impl RecurrenceBackend for TypeDBRecurrence {
    async fn load(&self, ctx: &QueueCtx, id: &str) -> QueueResult<Option<ScheduleRecord>> {
        if ctx.tenant_id != self.store.tenant {
            return Err(QueueError::InvalidConfig("Schedule tenant mismatch".into()));
        }
        let Some(old) = self.store.get("schedule", id).await.map_err(error)? else {
            return Ok(None);
        };
        let s: Schedule = serde_json::from_value(old.value)?;
        Ok(Some(ScheduleRecord {
            id: s.id,
            revision: old.revision,
            cursor: IntervalCursor {
                every_seconds: s.every_seconds,
                next: s.next,
                sequence: s.sequence,
            },
            enabled: s.enabled && old.status == "scheduled",
            template: Template::new(&TriggerWorkflow {
                workflow: s.workflow,
                input: s.input,
            })?,
        }))
    }
    async fn commit(
        &self,
        ctx: &QueueCtx,
        old: &ScheduleRecord,
        next: &IntervalCursor,
        message: JobMessage,
    ) -> QueueResult<()> {
        if ctx.tenant_id != self.store.tenant {
            return Err(QueueError::InvalidConfig("Schedule tenant mismatch".into()));
        }
        let Some(record) = self.store.get("schedule", &old.id).await.map_err(error)? else {
            return Err(QueueError::InvalidConfig("Schedule unavailable".into()));
        };
        if record.revision != old.revision {
            return Err(QueueError::Internal(
                "Schedule changed; reload on redelivery".into(),
            ));
        }
        let mut s: Schedule = serde_json::from_value(record.value)?;
        s.next = next.next;
        s.sequence = next.sequence;
        let revision = uuid::Uuid::new_v4().to_string();
        let wake = outbox(
            &self.store,
            "schedule",
            &old.id,
            &revision,
            "scheduled",
            next.next,
        )
        .map_err(error)?;
        let occurrence = Persisted::new(&self.store.tenant, message, None);
        let occurrence =
            insert_clause_named(&self.store, &occurrence, "occurrence").map_err(error)?;
        let query=format!("{} $r isa automation_record,has auto_tenant {},has auto_kind \"schedule\",has auto_key {},has auto_revision {},has auto_status \"scheduled\";select $t,$r;update $r has auto_payload {},has auto_revision {},has auto_status \"scheduled\",has auto_when {};{wake}{occurrence}fetch {{'id':$r.auto_key}};",self.store.prefix(),q(&self.store.tenant),q(&self.store.key("schedule",&old.id)),q(&old.revision),q(&serde_json::to_string(&s)?),q(&revision),next.next);
        crate::storage::write_queue(&self.store.state, &query)
            .await
            .map_err(error)?;
        Ok(())
    }
}
