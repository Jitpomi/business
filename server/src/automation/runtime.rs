//! Workflow scheduling and human reconciliation; execution belongs to background.
use super::{
    engine::{now, Engine},
    model::{identifier, Run, Schedule},
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
