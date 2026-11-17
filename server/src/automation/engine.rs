use super::{
    connectors::{redact, Catalog, Invocation},
    model::*,
    store::{Record, Store},
    vault::Vault,
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
#[derive(Clone)]
pub struct Engine {
    pub store: Store,
    pub catalog: Arc<Catalog>,
    pub vault: Vault,
}
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
impl Engine {
    pub async fn connect(
        &self,
        id: &str,
        kind: &str,
        config: Value,
        secrets: Value,
    ) -> Result<Value> {
        identifier(id)?;
        ensure!(
            config.to_string().len() + secrets.to_string().len() <= 65536,
            "Connection too large"
        );
        let connector = self.catalog.get(kind)?;
        connector.validate(&config, &secrets)?;
        ensure!(
            redact(config.clone(), &secrets) == config,
            "Credentials belong in the secret fields"
        );
        let revision = uuid::Uuid::new_v4().to_string();
        let context = format!("{}/{id}/{revision}", self.store.tenant);
        let secret_ref = if secrets.is_null() {
            String::new()
        } else {
            self.vault.seal(&context, &secrets).await?
        };
        let c = Connection {
            id: id.into(),
            connector: kind.into(),
            revision,
            config,
            secret_ref,
            enabled: true,
            verified: false,
            verification: None,
        };
        self.store
            .insert("connection", id, &serde_json::to_value(&c)?, "enabled", 0)
            .await?;
        Ok(c.public())
    }
    pub async fn connection(&self, id: &str) -> Result<(Record, Connection, Value)> {
        identifier(id)?;
        let r =
            self.store.get("connection", id).await?.ok_or_else(|| {
                dog_core::DogError::not_found("Connection unavailable").into_anyhow()
            })?;
        let c: Connection = serde_json::from_value(r.value.clone())?;
        let secrets = if c.secret_ref.is_empty() {
            Value::Null
        } else {
            self.vault
                .open(
                    &format!("{}/{}/{}", self.store.tenant, c.id, c.revision),
                    &c.secret_ref,
                )
                .await?
        };
        Ok((r, c, secrets))
    }
    pub async fn verify_connection(&self, id: &str) -> Result<Value> {
        let (r, mut c, s) = self.connection(id).await?;
        ensure!(c.enabled, "Connection disabled");
        let connector = self.catalog.get(&c.connector)?;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            connector.verify(&c.config, &s),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Verification timed out"))?
        .map_err(|_| anyhow::anyhow!("Connection verification failed"))?;
        c.verified = true;
        c.verification = Some(connector.verification_method().into());
        self.store
            .cas(&r, &serde_json::to_value(&c)?, "enabled", 0)
            .await?;
        Ok(c.public())
    }
    pub async fn disconnect(&self, id: &str) -> Result<Value> {
        identifier(id)?;
        let r = self
            .store
            .get("connection", id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Connection unavailable"))?;
        let mut c: Connection = serde_json::from_value(r.value.clone())?;
        c.enabled = false;
        self.store
            .cas(&r, &serde_json::to_value(&c)?, "disabled", 0)
            .await?;
        Ok(c.public())
    }
    pub async fn publish(&self, mut w: Workflow, owner: &str) -> Result<Value> {
        ensure!(serde_json::to_vec(&w)?.len() <= 65536, "Workflow too large");
        w.validate()?;
        self.store.owner(owner).await?;
        identifier(&w.id)?;
        for step in w.steps.iter().filter(|_| w.enabled) {
            if let Step::Action {
                connection, action, ..
            } = step
            {
                let (_, c, _) = self.connection(connection).await?;
                ensure!(
                    c.enabled && c.verified,
                    "Verify connections before publishing workflows"
                );
                ensure!(
                    self.catalog
                        .get(&c.connector)?
                        .actions()
                        .iter()
                        .any(|a| &a.name == action),
                    "Action unavailable on this connection"
                );
            }
        }
        let previous = self.store.get("workflow", &w.id).await?;
        if let Some(old) = &previous {
            ensure!(
                old.value["workflow"]["revision"].as_str() == Some(w.revision.as_str()),
                "Workflow changed; inspect its current revision before publishing"
            );
        } else {
            ensure!(
                w.revision.is_empty(),
                "New workflows must not supply an existing revision"
            );
        }
        w.revision = uuid::Uuid::new_v4().to_string();
        let value = json!({"workflow":w,"owner":owner});
        // Keep the submitted version even if publication later loses a race or acknowledgement.
        self.store
            .insert("workflow_version", &w.revision, &value, "saved", now())
            .await?;
        if let Some(old) = previous {
            self.store.cas(&old, &value, "published", 0).await?;
        } else {
            self.store
                .insert("workflow", &w.id, &value, "published", 0)
                .await?;
        }
        Ok(value)
    }
    async fn prepare(&self, workflow: &str, key: &str, input: Value) -> Result<Run> {
        identifier(workflow)?;
        ensure!(
            !key.is_empty() && key.len() <= 200,
            "An idempotency key is required"
        );
        ensure!(input.to_string().len() <= 65536, "Event too large");
        let row = self
            .store
            .get("workflow", workflow)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Workflow unavailable"))?;
        let w: Workflow = serde_json::from_value(row.value["workflow"].clone())?;
        ensure!(w.enabled, "Workflow disabled");
        self.store
            .owner(
                row.value["owner"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Workflow owner missing"))?,
            )
            .await?;
        let mut connections = std::collections::BTreeMap::new();
        for s in &w.steps {
            if let Step::Action { connection, .. } = s {
                let (_, c, _) = self.connection(connection).await?;
                ensure!(c.enabled && c.verified, "Connection unavailable");
                connections.insert(connection.clone(), c.revision);
            }
        }
        Ok(Run {
            owner: row.value["owner"].as_str().unwrap_or("").into(),
            id: hash(&format!("{}\0{workflow}\0{key}", self.store.tenant)),
            workflow: w,
            input,
            outputs: Default::default(),
            cursor: 0,
            status: "queued".into(),
            wake: now(),
            attempt: None,
            approved: vec![],
            connections,
            last_error: None,
        })
    }
    pub async fn enqueue(&self, workflow: &str, key: &str, input: Value) -> Result<Value> {
        identifier(workflow)?;
        ensure!(
            !key.is_empty() && key.len() <= 200,
            "An idempotency key is required"
        );
        ensure!(input.to_string().len() <= 65536, "Event too large");
        let id = hash(&format!("{}\0{workflow}\0{key}", self.store.tenant));
        if let Some(old) = self.store.get("run", &id).await? {
            let old_run: Run = serde_json::from_value(old.value.clone())?;
            ensure!(
                old_run.input == input,
                "Idempotency key reused with different input"
            );
            return Ok(old.value);
        }
        let run = self.prepare(workflow, key, input).await?;
        match self
            .store
            .insert(
                "run",
                &run.id,
                &serde_json::to_value(&run)?,
                "queued",
                run.wake,
            )
            .await
        {
            Ok(r) => Ok(r.value),
            Err(e) => {
                if let Some(existing) = self.store.get("run", &run.id).await? {
                    let r: Run = serde_json::from_value(existing.value.clone())?;
                    ensure!(r.input == run.input, "Conflicting event replay");
                    Ok(existing.value)
                } else {
                    Err(e)
                }
            }
        }
    }
    async fn save(&self, old: &Record, run: &Run) -> Result<Record> {
        self.store
            .cas(old, &serde_json::to_value(run)?, &run.status, run.wake)
            .await
    }
    async fn failure_decision(&self, claim: &Record, oversized: bool) -> Result<String> {
        let rows=crate::storage::read(&self.store.state,&format!("{} $r isa automation_record,has auto_key {},has auto_revision {};let $decision=automation_failure_decision($r,{oversized});fetch {{'decision':$decision}};",self.store.prefix(),super::store::q(&claim.key),super::store::q(&claim.revision))).await?;
        Ok(rows
            .first()
            .and_then(|row| row["decision"].as_str())
            .ok_or_else(|| anyhow::anyhow!("Native outcome policy unavailable; claim retained"))?
            .into())
    }
    pub async fn approve(&self, id: &str, actor: &str) -> Result<Value> {
        self.store.owner(actor).await?;
        let old = self
            .store
            .get("run", id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Run unavailable"))?;
        let mut run: Run = serde_json::from_value(old.value.clone())?;
        let step = run
            .workflow
            .steps
            .get(run.cursor)
            .ok_or_else(|| {
                dog_core::DogError::conflict("Run has no current approval step").into_anyhow()
            })?
            .id()
            .to_owned();
        run.approved.push(step.clone());
        run.outputs
            .insert(step, json!({"approved_by":actor,"at":now()}));
        run.status = "queued".into();
        run.wake = now();
        Ok(self
            .store
            .control(
                &old,
                &serde_json::to_value(&run)?,
                &run.status,
                run.wake,
                actor,
                "approve",
            )
            .await?
            .value)
    }
    pub async fn cancel(&self, id: &str) -> Result<Value> {
        let old = self
            .store
            .get("run", id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Run unavailable"))?;
        let mut r: Run = serde_json::from_value(old.value.clone())?;
        r.status = "cancelled".into();
        let actor = self.store.request_actor()?;
        Ok(self
            .store
            .control(
                &old,
                &serde_json::to_value(&r)?,
                &r.status,
                r.wake,
                actor,
                "cancel",
            )
            .await?
            .value)
    }
    pub async fn advance(&self, id: &str) -> Result<Value> {
        let old = self
            .store
            .get("run", id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Run unavailable"))?;
        let mut run: Run = serde_json::from_value(old.value.clone())?;
        let rows=crate::storage::read(&self.store.state,&format!("{} $r isa automation_record,has auto_key {};fetch {{'runnable':automation_runnable($r,{})}};",self.store.prefix(),super::store::q(&old.key),now())).await?;
        if !rows.first().is_some_and(|row| row["runnable"] == true) {
            return Ok(old.value);
        }
        if run.cursor >= run.workflow.steps.len() {
            run.status = "completed".into();
            return Ok(self.save(&old, &run).await?.value);
        }
        if let Err(cause) = self.store.owner(&run.owner).await {
            if !cause
                .downcast_ref::<dog_core::DogError>()
                .is_some_and(|e| e.kind == dog_core::ErrorKind::Forbidden)
            {
                return Err(cause);
            }
            run.status = "blocked".into();
            run.last_error = Some("owner_permission_changed".into());
            return Ok(self.save(&old, &run).await?.value);
        }
        run.status = "queued".into();
        let step = match run.workflow.steps[run.cursor].resolve(&run.input, &run.outputs) {
            Ok(step) => step,
            Err(_) => {
                run.status = "failed".into();
                run.last_error = Some("invalid_input_reference".into());
                return Ok(self.save(&old, &run).await?.value);
            }
        };
        let step_id = step.id().to_owned();
        match step {
            Step::Transform { id, value } => {
                run.outputs.insert(id, value);
                run.cursor += 1;
            }
            Step::Condition { id, left, equals } => {
                let matches = left == equals;
                run.outputs.insert(id, json!(matches));
                if matches {
                    run.cursor += 1;
                } else {
                    run.status = "filtered".into();
                }
            }
            Step::Delay { id, seconds } => {
                run.wake =
                    dog_queue::timing::delayed_at(chrono::Utc::now(), u64::try_from(seconds)?)?
                        .timestamp();
                run.status = "waiting".into();
                run.outputs.insert(id, json!({"resume_at":run.wake}));
                run.cursor += 1;
            }
            Step::Approval { id } => {
                if run.approved.contains(&id) {
                    run.cursor += 1;
                } else {
                    run.status = "approval".into();
                }
            }
            Step::Action {
                id,
                connection,
                action,
                input,
            } => {
                let (connection_record, c, secrets) = match self.connection(&connection).await {
                    Ok(c) => c,
                    Err(cause) => {
                        if !cause
                            .downcast_ref::<dog_core::DogError>()
                            .is_some_and(|e| e.kind == dog_core::ErrorKind::NotFound)
                        {
                            return Err(cause);
                        }
                        run.status = "blocked".into();
                        run.last_error = Some("connection_unavailable".into());
                        return Ok(self.save(&old, &run).await?.value);
                    }
                };
                if !c.enabled
                    || !c.verified
                    || run.connections.get(&connection) != Some(&c.revision)
                {
                    run.status = "blocked".into();
                    run.last_error = Some("connection_unavailable".into());
                    return Ok(self.save(&old, &run).await?.value);
                }
                let connector = match self.catalog.get(&c.connector) {
                    Ok(c) => c,
                    Err(_) => {
                        run.status = "blocked".into();
                        run.last_error = Some("connector_unavailable".into());
                        return Ok(self.save(&old, &run).await?.value);
                    }
                };
                let Some(spec) = connector.actions().into_iter().find(|s| s.name == action) else {
                    run.status = "blocked".into();
                    run.last_error = Some("action_unavailable".into());
                    return Ok(self.save(&old, &run).await?.value);
                };
                let effect = spec.effect;
                let previous = run.attempt.as_ref().map(|a| a.count).unwrap_or(0);
                let rows=crate::storage::read(&self.store.state,&format!("{} $r isa automation_record,has auto_key {},has auto_tenant {};let $decision=automation_recovery_decision($r,{});fetch {{'decision':$decision}};",self.store.prefix(),super::store::q(&old.key),super::store::q(&self.store.tenant),serde_json::to_value(effect)?)).await?;
                let decision = rows
                    .first()
                    .and_then(|v| v["decision"].as_str())
                    .ok_or_else(|| anyhow::anyhow!("Native execution policy unavailable"))?;
                if decision != "claim" {
                    run.status = decision.into();
                    run.last_error = Some(
                        if decision == "attention" {
                            "uncertain_external_outcome"
                        } else {
                            "retry_limit"
                        }
                        .into(),
                    );
                    return Ok(self.save(&old, &run).await?.value);
                }
                if connector
                    .validate_input(&c.config, &action, &input)
                    .is_err()
                {
                    run.status = "failed".into();
                    run.last_error = Some("invalid_action_input".into());
                    return Ok(self.save(&old, &run).await?.value);
                }
                let key = hash(&format!("{}\0{}\0{id}", self.store.tenant, run.id));
                run.attempt = Some(Attempt {
                    key: key.clone(),
                    count: previous + 1,
                    effect,
                    started: now(),
                });
                run.status = "running".into();
                run.wake = dog_queue::timing::delayed_at(
                    chrono::Utc::now(),
                    connector
                        .timeout_seconds()
                        .checked_add(60)
                        .ok_or_else(|| anyhow::anyhow!("Action deadline exhausted"))?,
                )?
                .timestamp();
                let claimed = self
                    .store
                    .claim(
                        &old,
                        &serde_json::to_value(&run)?,
                        &run.status,
                        run.wake,
                        &run.owner,
                        &connection_record,
                    )
                    .await?;
                let ctx = Invocation {
                    tenant: self.store.tenant.clone(),
                    run: run.id.clone(),
                    step: id.clone(),
                    key,
                    config: c.config,
                    secrets: secrets.clone(),
                };
                let outcome = tokio::time::timeout(
                    std::time::Duration::from_secs(connector.timeout_seconds()),
                    connector.execute(&ctx, &action, input),
                )
                .await;
                match outcome {
                    Ok(Ok(output)) => {
                        let output = redact(output, &secrets);
                        if output.to_string().len() > 262144
                            || serde_json::to_vec(&run.outputs)?.len()
                                + serde_json::to_vec(&output)?.len()
                                + id.len()
                                + 4
                                > 1_048_576
                        {
                            run.status = self.failure_decision(&claimed, true).await?;
                            run.last_error = Some("output_limit".into());
                            return Ok(self.save(&claimed, &run).await?.value);
                        }
                        run.outputs.insert(id.clone(), output);
                        run.cursor += 1;
                        run.attempt = None;
                        run.status = "queued".into();
                        run.last_error = None;
                        run.wake = now();
                    }
                    _ => {
                        run.last_error = Some("connector_action_failed".into());
                        run.status = self.failure_decision(&claimed, false).await?;
                        if run.status == "attention" {
                            run.wake = 0;
                        } else {
                            run.wake = dog_queue::timing::retry_at(
                                chrono::Utc::now(),
                                previous,
                                1,
                                64,
                                false,
                            )?
                            .timestamp();
                        }
                    }
                }
                if run.cursor == run.workflow.steps.len()
                    && run.attempt.is_none()
                    && run.status == "queued"
                {
                    run.status = "completed".into();
                }
                return Ok(self.save(&claimed, &run).await?.value);
            }
        }
        if serde_json::to_vec(&run.outputs)?.len() > 1_048_576 {
            run.status = "failed".into();
            run.last_error = Some("run_output_limit".into());
            run.outputs.remove(&step_id);
        }
        if run.cursor == run.workflow.steps.len() && run.status == "queued" {
            run.status = "completed".into();
        }
        Ok(self.save(&old, &run).await?.value)
    }
}
