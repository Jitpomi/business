//! Durable polling. A schedule advances only after its deterministic event exists.
use super::{
    engine::{now, Engine},
    model::{identifier, Run, Schedule},
    store::{q, Store},
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
impl Engine {
    pub async fn schedule(&self, mut s: Schedule) -> Result<Value> {
        identifier(&s.id)?;
        identifier(&s.workflow)?;
        ensure!(
            (60..=31_536_000).contains(&s.every_seconds),
            "Schedule interval must be at least 60 seconds"
        );
        ensure!(s.input.to_string().len() <= 65536, "Event too large");
        ensure!(
            self.store.get("workflow", &s.workflow).await?.is_some(),
            "Workflow unavailable"
        );
        s.sequence = 0;
        let value = serde_json::to_value(&s)?;
        if let Some(old) = self.store.get("schedule", &s.id).await? {
            let previous: Schedule = serde_json::from_value(old.value.clone())?;
            ensure!(
                s.workflow == previous.workflow
                    && s.input == previous.input
                    && s.every_seconds == previous.every_seconds,
                "Create a new schedule ID when changing its event or interval"
            );
            s.next = previous.next;
            s.sequence = previous.sequence;
            return Ok(self
                .store
                .cas(
                    &old,
                    &serde_json::to_value(&s)?,
                    if s.enabled { "scheduled" } else { "disabled" },
                    s.next,
                )
                .await?
                .value);
        }
        Ok(self
            .store
            .insert(
                "schedule",
                &s.id,
                &value,
                if s.enabled { "scheduled" } else { "disabled" },
                s.next,
            )
            .await?
            .value)
    }
    pub async fn fire_schedule(&self, id: &str) -> Result<()> {
        let old = self
            .store
            .get("schedule", id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Schedule unavailable"))?;
        let mut s: Schedule = serde_json::from_value(old.value.clone())?;
        if !s.enabled || s.next > now() {
            return Ok(());
        }
        if self
            .enqueue(
                &s.workflow,
                &format!("schedule:{}:{}", s.id, s.sequence),
                s.input.clone(),
            )
            .await
            .is_err()
        {
            // A disabled workflow, revoked owner or unavailable connection must not
            // occupy the front of the due queue forever. Owner review resumes it.
            self.store.cas(&old, &old.value, "blocked", 0).await?;
            return Ok(());
        }
        s.sequence = s
            .sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Schedule exhausted"))?;
        s.next = s
            .next
            .checked_add(s.every_seconds)
            .ok_or_else(|| anyhow::anyhow!("Schedule exhausted"))?;
        self.store
            .cas(&old, &serde_json::to_value(s.clone())?, "scheduled", s.next)
            .await?;
        Ok(())
    }
    /// Resolving an uncertain write records evidence; it never sends the request again.
    pub async fn resolve(
        &self,
        id: &str,
        actor: &str,
        evidence: &str,
        output: Value,
    ) -> Result<Value> {
        self.store.owner(actor).await?;
        ensure!(
            (8..=2000).contains(&evidence.len()),
            "Record how the external outcome was verified"
        );
        ensure!(output.to_string().len() <= 65536, "Resolution too large");
        let old = self
            .store
            .get("run", id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Run unavailable"))?;
        let mut r: Run = serde_json::from_value(old.value.clone())?;
        ensure!(
            r.status == "attention",
            "Run does not require reconciliation"
        );
        let step = r
            .workflow
            .steps
            .get(r.cursor)
            .ok_or_else(|| anyhow::anyhow!("Run cursor invalid"))?
            .id()
            .to_owned();
        ensure!(
            serde_json::to_vec(&r.outputs)?.len() + output.to_string().len() + evidence.len() + 512
                <= 1_048_576,
            "Resolution exceeds run output limit"
        );
        r.outputs.insert(
            step,
            json!({"verified_by":actor,"evidence":evidence,"output":output,"at":now()}),
        );
        r.cursor += 1;
        r.attempt = None;
        r.last_error = None;
        r.status = "queued".into();
        r.wake = now();
        Ok(self
            .store
            .cas(&old, &serde_json::to_value(&r)?, &r.status, r.wake)
            .await?
            .value)
    }
    pub async fn resume(&self, id: &str) -> Result<Value> {
        let old = self
            .store
            .get("run", id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Run unavailable"))?;
        let mut r: Run = serde_json::from_value(old.value.clone())?;
        ensure!(r.status == "blocked", "Only blocked runs can resume");
        r.status = "queued".into();
        r.wake = now();
        Ok(self
            .store
            .cas(&old, &serde_json::to_value(&r)?, &r.status, r.wake)
            .await?
            .value)
    }
}
impl Store {
    pub async fn due(&self) -> Result<Vec<(String, String)>> {
        let rows=crate::storage::read(&self.state,&format!("{} $r isa automation_record,has auto_tenant {},has auto_key $key,has auto_kind $kind,has auto_status $status,has auto_when $when;$when <= {};{{$kind == \"run\";{{$status == \"queued\";}} or {{$status == \"running\";}} or {{$status == \"retry\";}} or {{$status == \"waiting\";}};}} or {{$kind == \"schedule\";$status == \"scheduled\";}};select $r,$when,$key;sort $when,$key;limit 32;fetch {{'key':$r.auto_key,'kind':$r.auto_kind}};",self.prefix(),q(&self.tenant),now())).await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some((
                    r["kind"].as_str()?.into(),
                    r["key"].as_str()?.rsplit('/').next()?.into(),
                ))
            })
            .collect())
    }
}
pub async fn tick(engine: &Engine) -> Result<usize> {
    let due = engine.store.due().await?;
    let n = due.len();
    for (kind, id) in due.into_iter().take(4) {
        let result = if kind == "run" {
            engine.advance(&id).await.map(|_| ())
        } else {
            engine.fire_schedule(&id).await
        };
        if result.is_err() {
            tracing::warn!(tenant=%engine.store.tenant,record=%id,"Automation deferred; durable state retained");
        }
    }
    Ok(n)
}
/// Owns a poller. Explicit shutdown waits for its current poll and effects to finish.
/// Dropping the handle requests a stop; callers retiring a database must await shutdown.
#[must_use = "Retain the worker and await shutdown before retiring its database"]
pub struct Worker {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Worker {
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await?;
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
/// Discover tenants with due records on every poll; newly connected tenants need no restart.
pub fn spawn_all(template: Engine) -> Worker {
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut clock = tokio::time::interval(std::time::Duration::from_secs(2));
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // Stop only between polls. Cancelling a query or provider mutation
            // midway can conceal its outcome; finish and persist it first.
            tokio::select! {
                biased;
                _ = &mut stopped => break,
                _ = clock.tick() => {},
            }
            let query=format!("match $r isa automation_record,has auto_tenant $tenant,has auto_when $when,has auto_status $status;$when <= {};{{$status == \"queued\";}} or {{$status == \"running\";}} or {{$status == \"retry\";}} or {{$status == \"waiting\";}} or {{$status == \"scheduled\";}};select $tenant;distinct;limit 1000;fetch {{'tenant':$tenant}};",now());
            let Ok(rows) = crate::storage::read(&template.store.state, &query).await else {
                tracing::warn!("Automation discovery failed");
                continue;
            };
            use futures::StreamExt;
            futures::stream::iter(rows)
                .for_each_concurrent(4, |row| {
                    let template = template.clone();
                    async move {
                        let Some(tenant) = row["tenant"].as_str() else {
                            return;
                        };
                        let Ok(store) = Store::new(template.store.state.clone(), tenant) else {
                            return;
                        };
                        let e = Engine {
                            store,
                            catalog: template.catalog.clone(),
                            vault: template.vault.clone(),
                        };
                        if tick(&e).await.is_err() {
                            tracing::warn!(tenant=%tenant,"Automation tenant polling failed");
                        }
                    }
                })
                .await;
        }
    });
    Worker {
        stop: Some(stop),
        task: Some(task),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/automation_worker.rs"]
mod worker_tests;
