//! Durable dog-queue ledger. All writes are tenant-qualified, revision-fenced TypeDB transactions.
//! Uses the existing private automation record schema; no customer data is rewritten.
use crate::automation::store::{q, Record, Store};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dog_queue::{backend::BoxStream, *};
use serde::{Deserialize, Serialize};
use std::time::Duration;
#[derive(Clone)]
pub struct TypeDBBackend {
    pub store: Store,
    pub lease_duration: Duration,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Persisted {
    pub record: JobRecord,
    lease: Option<LeaseToken>,
}
impl Persisted {
    pub fn new(tenant: &str, message: JobMessage, id: Option<&str>) -> Self {
        Self {
            record: JobRecord::new(id.map(JobId::from).unwrap_or_default(), tenant, message),
            lease: None,
        }
    }
    fn due(&self) -> i64 {
        match &self.record.status {
            JobStatus::Processing { lease_until } => lease_until.timestamp(),
            JobStatus::Retrying { retry_at } => retry_at.timestamp(),
            _ => self.record.message.run_at.timestamp(),
        }
    }
}
pub(crate) fn insert_clause(store: &Store, value: &Persisted) -> anyhow::Result<String> {
    insert_clause_named(store, value, "job")
}
pub(crate) fn insert_clause_named(
    store: &Store,
    value: &Persisted,
    variable: &str,
) -> anyhow::Result<String> {
    Ok(format!("insert ${variable} isa automation_record,has auto_key {},has auto_tenant {},has auto_kind \"job\",has auto_payload {},has auto_revision {},has auto_status {},has auto_when {};",q(&store.key("job",value.record.job_id.as_str())),q(&store.tenant),q(&serde_json::to_string(value)?),q(&uuid::Uuid::new_v4().to_string()),q(value.record.status.name()),value.due()))
}
fn error(cause: anyhow::Error) -> QueueError {
    // Preserve safe machine diagnostics without logging queries, payloads or tokens.
    let code = if let Some(error) = cause.downcast_ref::<typedb_driver::Error>() {
        error.code()
    } else if let Some(error) = cause.downcast_ref::<dog_core::DogError>() {
        format!("{:?}", error.kind)
    } else if cause.is::<tokio::time::error::Elapsed>() {
        "deadline".into()
    } else {
        "internal".into()
    };
    QueueError::Internal(format!(
        "TypeDB queue transaction failed ({code}); durable state retained"
    ))
}
impl TypeDBBackend {
    fn check(&self, ctx: &QueueCtx) -> QueueResult<()> {
        if ctx.tenant_id != self.store.tenant {
            return Err(QueueError::InvalidConfig("Queue tenant mismatch".into()));
        }
        Ok(())
    }
    async fn load(&self, ctx: &QueueCtx, id: &JobId) -> QueueResult<(Record, Persisted)> {
        self.check(ctx)?;
        let old = self
            .store
            .get("job", id.as_str())
            .await
            .map_err(error)?
            .ok_or_else(|| QueueError::JobNotFound(id.clone()))?;
        let value: Persisted = serde_json::from_value(old.value.clone())?;
        if value.record.tenant_id != ctx.tenant_id || value.record.job_id != *id {
            return Err(QueueError::JobNotFound(id.clone()));
        }
        Ok((old, value))
    }
    async fn save(&self, old: &Record, value: &Persisted) -> QueueResult<()> {
        self.store
            .cas(
                old,
                &serde_json::to_value(value)?,
                value.record.status.name(),
                value.due(),
            )
            .await
            .map_err(error)?;
        Ok(())
    }
    /// Reconcile metadata CAS races, including a lost commit acknowledgement.
    /// Never repeats a handler or a business/provider operation.
    async fn save_leased(&self, original: &Record, desired: &Persisted) -> QueueResult<()> {
        let initial: Persisted = serde_json::from_value(original.value.clone())?;
        let token = initial.lease.ok_or_else(|| QueueError::InvalidLeaseToken {
            job_id: desired.record.job_id.clone(),
        })?;
        let ctx = QueueCtx::new(&self.store.tenant);
        let mut old = original.clone();
        let mut value = desired.clone();
        for _ in 0..3 {
            let encoded = serde_json::to_value(&value)?;
            let guard=format!("$held isa automation_record,has auto_key {},has auto_tenant {},has auto_revision {},has auto_status \"processing\",has auto_when $deadline;$deadline > {};",q(&old.key),q(&self.store.tenant),q(&old.revision),Utc::now().timestamp());
            match self
                .store
                .cas_guarded(
                    &old,
                    &encoded,
                    value.record.status.name(),
                    value.due(),
                    &guard,
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(_cause) => {
                    let (current, held) = self.load(&ctx, &value.record.job_id).await?;
                    if current.value == encoded {
                        return Ok(());
                    } // committed; acknowledgement lost
                    Self::validate(&held, &token)?;
                    // A fresh read proved this exact lease still owns the record.
                    // Retry only its fenced metadata transition, even if an unrelated
                    // commit lock aborted the write without changing this job revision.
                    // This cannot repeat a handler or overwrite a later lease.
                    // A heartbeat may also have changed this same owned lease; never
                    // shorten a newer renewal.
                    if let (Some(wanted), Some(actual)) =
                        (value.record.lease_until(), held.record.lease_until())
                    {
                        value
                            .record
                            .start_processing(token.clone(), wanted.max(actual));
                    }
                    old = current;
                }
            }
        }
        Err(QueueError::Internal(
            "Queue metadata remained contended; durable lease retained".into(),
        ))
    }
    async fn save_candidate(
        &self,
        ctx: &QueueCtx,
        old: &Record,
        value: &Persisted,
    ) -> QueueResult<bool> {
        match self.save(old, value).await {
            Ok(()) => Ok(true),
            Err(cause) => {
                let (current, _) = self.load(ctx, &value.record.job_id).await?;
                if current.value == serde_json::to_value(value)? {
                    return Ok(true);
                }
                if current.revision != old.revision {
                    // Another consumer won this exact job fence.
                    return Ok(false);
                }
                // A failed or unacknowledged claim is never executed speculatively.
                Err(cause)
            }
        }
    }
    fn validate(value: &Persisted, token: &LeaseToken) -> QueueResult<()> {
        if matches!(value.record.status, JobStatus::Canceled { .. }) {
            return Err(QueueError::JobCanceled);
        }
        if value.record.status.is_terminal() {
            return Err(QueueError::JobAlreadyTerminal);
        }
        if value.lease.as_ref() != Some(token) {
            return Err(QueueError::InvalidLeaseToken {
                job_id: value.record.job_id.clone(),
            });
        }
        if !value
            .record
            .lease_until()
            .is_some_and(|until| until > Utc::now())
        {
            return Err(QueueError::LeaseExpired);
        }
        Ok(())
    }
}
#[async_trait]
impl QueueBackend for TypeDBBackend {
    async fn enqueue(&self, ctx: QueueCtx, message: JobMessage) -> QueueResult<JobId> {
        self.check(&ctx)?;
        // Workflow outbox deliveries use revision IDs. Generic submissions use an
        // immutable hash of tenant/queue/type/key for tenant-scoped deduplication.
        use sha2::{Digest, Sha256};
        let id = message.idempotency_key.as_ref().map(|key| {
            format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(&ctx.tenant_id, &message.queue, &message.job_type, key))
                        .expect("strings serialize")
                )
            )
        });
        let value = Persisted::new(&ctx.tenant_id, message, id.as_deref());
        let id = value.record.job_id.clone();
        if self
            .store
            .get("job", id.as_str())
            .await
            .map_err(error)?
            .is_some()
        {
            return Ok(id);
        }
        let query = format!("{} not {{$existing isa automation_record,has auto_key {};}};select $t;{}fetch {{'id':$job.auto_key}};",self.store.prefix(),q(&self.store.key("job",id.as_str())),insert_clause(&self.store,&value).map_err(error)?);
        if let Err(e) = crate::storage::write_queue(&self.store.state, &query).await {
            // A lost acknowledgement is reconciled by the immutable ID, never a new submission.
            if self
                .store
                .get("job", id.as_str())
                .await
                .map_err(error)?
                .is_none()
            {
                return Err(error(e));
            }
        }
        Ok(id)
    }
    async fn dequeue(&self, ctx: QueueCtx, queues: &[&str]) -> QueueResult<Option<LeasedJob>> {
        self.check(&ctx)?;
        let rows = crate::storage::read(&self.store.state,&format!("{} $r isa automation_record,has auto_tenant {},has auto_kind \"job\",has auto_key $key,has auto_when $when,has auto_status $status;$when <= {};{{$status == \"enqueued\";}} or {{$status == \"retrying\";}} or {{$status == \"processing\";}};select $r,$when,$key;sort $when,$key;limit 32;fetch {{'id':$r.auto_key}};",self.store.prefix(),q(&ctx.tenant_id),Utc::now().timestamp())).await.map_err(error)?;
        for row in rows {
            let id = row["id"]
                .as_str()
                .and_then(|s| s.rsplit('/').next())
                .ok_or_else(|| QueueError::Internal("Invalid queue ID".into()))?;
            let (old, mut value) = self.load(&ctx, &JobId::from(id)).await?;
            if !queues.contains(&value.record.message.queue.as_str())
                || value.due() > Utc::now().timestamp()
            {
                continue;
            }
            if value.record.status.is_processing() {
                // Recovery redelivers the revision reference, never a raw payment.
                // The workflow's effect journal decides whether retry is safe.
                if !value.record.lease_expired(Utc::now()) {
                    continue;
                }
                if value.record.attempt > value.record.message.max_retries {
                    value
                        .record
                        .fail("Delivery lease expired; retry budget exhausted".into());
                } else {
                    value.record.schedule_retry(Utc::now());
                }
                value.lease = None;
                self.save_candidate(&ctx, &old, &value).await?;
                continue;
            }
            if !value.record.status.is_eligible(Utc::now())
                || !value.record.message.is_eligible(Utc::now())
            {
                continue;
            }
            let token = LeaseToken::new();
            let until = Utc::now()
                + chrono::Duration::from_std(self.lease_duration)
                    .map_err(|_| QueueError::InvalidConfig("Invalid lease duration".into()))?;
            value.record.attempt = value
                .record
                .attempt
                .checked_add(1)
                .ok_or_else(|| QueueError::Internal("Queue attempt limit".into()))?;
            value.record.start_processing(token.clone(), until);
            value.lease = Some(token.clone());
            if !self.save_candidate(&ctx, &old, &value).await? {
                continue;
            }
            return Ok(Some(LeasedJob::new(value.record, token, until)));
        }
        Ok(None)
    }
    async fn ack_complete(
        &self,
        ctx: QueueCtx,
        id: JobId,
        token: LeaseToken,
        result: Option<String>,
    ) -> QueueResult<()> {
        let (old, mut value) = self.load(&ctx, &id).await?;
        Self::validate(&value, &token)?;
        value.record.complete();
        value.record.result = result;
        value.lease = None;
        self.save_leased(&old, &value).await
    }
    async fn ack_fail(
        &self,
        ctx: QueueCtx,
        id: JobId,
        token: LeaseToken,
        reason: String,
        retry: Option<DateTime<Utc>>,
    ) -> QueueResult<()> {
        let (old, mut value) = self.load(&ctx, &id).await?;
        Self::validate(&value, &token)?;
        value.record.set_error(reason.clone());
        if let Some(at) = retry {
            value.record.schedule_retry(at);
        } else {
            value.record.fail(reason);
        }
        value.lease = None;
        self.save_leased(&old, &value).await
    }
    async fn heartbeat_extend(
        &self,
        ctx: QueueCtx,
        id: JobId,
        token: LeaseToken,
        extra: Duration,
    ) -> QueueResult<()> {
        let (old, mut value) = self.load(&ctx, &id).await?;
        Self::validate(&value, &token)?;
        let until = Utc::now()
            + chrono::Duration::from_std(extra)
                .map_err(|_| QueueError::InvalidConfig("Invalid extension".into()))?;
        value
            .record
            .start_processing(token, until.max(value.record.lease_until().unwrap()));
        self.save_leased(&old, &value).await
    }
    async fn cancel(&self, ctx: QueueCtx, id: JobId) -> QueueResult<bool> {
        let (old, mut value) = self.load(&ctx, &id).await?;
        if value.record.status.is_terminal() {
            return Ok(false);
        }
        value.record.cancel();
        value.lease = None;
        self.save(&old, &value).await?;
        Ok(true)
    }
    async fn get_status(&self, ctx: QueueCtx, id: JobId) -> QueueResult<JobStatus> {
        Ok(self.load(&ctx, &id).await?.1.record.status)
    }
    async fn get_record(&self, ctx: QueueCtx, id: JobId) -> QueueResult<JobRecord> {
        Ok(self.load(&ctx, &id).await?.1.record)
    }
    fn event_stream(&self, _ctx: QueueCtx) -> BoxStream<JobEvent> {
        Box::pin(futures::stream::empty())
    }
    fn capabilities(&self) -> QueueCapabilities {
        QueueCapabilities {
            delayed: true,
            scheduled_at: true,
            cancel: true,
            lease_extend: true,
            idempotency: true,
            ..Default::default()
        }
    }
}
