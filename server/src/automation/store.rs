//! Tenant-qualified durable records and compare-and-swap claims.
use crate::{storage, typedb::TypeDBState};
use anyhow::{ensure, Result};
use serde_json::Value;
use std::sync::Arc;
pub const SCHEMA: &str = include_str!("schema.tql");
pub const POLICY_SCHEMA: &str = include_str!("policy-schema.tql");
pub fn q(v: &str) -> String {
    serde_json::to_string(v).unwrap()
}
#[derive(Clone)]
pub struct Store {
    pub state: Arc<TypeDBState>,
    pub tenant: String,
    actor: Option<String>,
    origin: Option<(String, String, String)>,
}
#[derive(Clone)]
pub struct Record {
    pub key: String,
    pub revision: String,
    pub value: Value,
    pub status: String,
    pub when: i64,
}
impl Store {
    pub async fn summaries(&self, kind: &str, limit: usize) -> Result<Vec<Value>> {
        self.page(kind, None, limit).await
    }
    pub async fn page(&self, kind: &str, after: Option<&str>, limit: usize) -> Result<Vec<Value>> {
        ensure!((1..=100).contains(&limit), "Summary limit must be 1..100");
        let cursor = if let Some(after) = after {
            ensure!(
                after.len() <= 512 && after.starts_with(&format!("{}/{kind}/", self.tenant)),
                "Cursor does not belong to this tenant and record kind"
            );
            format!("$key > {};", q(after))
        } else {
            String::new()
        };
        crate::storage::read(&self.state,&format!("{} $r isa automation_record,has auto_tenant {},has auto_kind {},has auto_key $key;{cursor}select $r,$key;sort $key;limit {limit};fetch {{'key':$r.auto_key,'revision':$r.auto_revision,'status':$r.auto_status,'due':$r.auto_when}};",self.prefix(),q(&self.tenant),q(kind))).await
    }

    pub fn new(state: Arc<TypeDBState>, tenant: &str) -> Result<Self> {
        super::model::identifier(tenant).map_err(|_| {
            dog_core::DogError::bad_request("Invalid tenant identifier").into_anyhow()
        })?;
        Ok(Self {
            state,
            tenant: tenant.into(),
            actor: None,
            origin: None,
        })
    }
    /// Trusted request identity; authorization is re-evaluated by TypeDB on every
    /// transaction, including the write after an asynchronous provider operation.
    pub fn for_owner(&self, actor: &str) -> Result<Self> {
        ensure!(
            !actor.is_empty() && actor.len() <= 512,
            "Invalid authenticated identity"
        );
        Ok(Self {
            state: self.state.clone(),
            tenant: self.tenant.clone(),
            actor: Some(actor.into()),
            origin: self.origin.clone(),
        })
    }
    pub(crate) fn for_origin(&self, record: &Record) -> Result<Self> {
        let kind = record
            .key
            .rsplit('/')
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Invalid origin"))?;
        ensure!(
            record.key.starts_with(&format!("{}/", self.tenant)),
            "Origin tenant mismatch"
        );
        let status = match kind {
            "schedule" => "scheduled",
            "trigger" => "enabled",
            _ => anyhow::bail!("Invalid event origin"),
        };
        let mut scoped = self.clone();
        scoped.origin = Some((
            record.key.clone(),
            status.into(),
            super::policy::origin_definition(kind, &record.value)?,
        ));
        Ok(scoped)
    }
    pub(crate) fn request_actor(&self) -> Result<&str> {
        self.actor.as_deref().ok_or_else(|| {
            dog_core::DogError::not_authenticated("Authenticated owner context required")
                .into_anyhow()
        })
    }
    pub fn prefix(&self) -> String {
        let scope = self
            .actor
            .as_ref()
            .map(|actor| {
                format!(
                    "let $authorized=automation_owner_scope({},{},{},$t);$authorized==true;",
                    q(&self.tenant),
                    q(actor),
                    native_now()
                )
            })
            .unwrap_or_default();
        let origin=self.origin.as_ref().map(|(key,status,definition)|format!("$origin isa automation_record,has auto_tenant {},has auto_key {},has auto_status {},has auto_enabled true,has auto_definition {};",q(&self.tenant),q(key),q(status),q(definition))).unwrap_or_default();
        format!(
            "match $t isa tenant,has tenant_id {};{scope}{origin}",
            q(&self.tenant)
        )
    }
    pub fn key(&self, kind: &str, id: &str) -> String {
        format!("{}/{kind}/{id}", self.tenant)
    }
    pub async fn get(&self, kind: &str, id: &str) -> Result<Option<Record>> {
        let rows=storage::read(&self.state,&format!("{} $r isa automation_record,has auto_tenant {},has auto_key {};fetch {{'key':$r.auto_key,'revision':$r.auto_revision,'payload':$r.auto_payload,'status':$r.auto_status,'when':$r.auto_when}};",self.prefix(),q(&self.tenant),q(&self.key(kind,id)))).await?;
        rows.first().map(record).transpose()
    }
    pub async fn list(&self, kind: &str, limit: usize) -> Result<Vec<Record>> {
        ensure!(limit <= 1000, "Record limit exceeded");
        storage::read(&self.state,&format!("{} $r isa automation_record,has auto_tenant {},has auto_kind {},has auto_key $key;select $r,$key;sort $key;limit {limit};fetch {{'key':$r.auto_key,'revision':$r.auto_revision,'payload':$r.auto_payload,'status':$r.auto_status,'when':$r.auto_when}};",self.prefix(),q(&self.tenant),q(kind))).await?.iter().map(record).collect()
    }
    pub async fn insert(
        &self,
        kind: &str,
        id: &str,
        value: &Value,
        status: &str,
        when: i64,
    ) -> Result<Record> {
        let key = self.key(kind, id);
        let rev = uuid::Uuid::new_v4().to_string();
        let delivery = crate::background::jobs::outbox(self, kind, id, &rev, status, when)?;
        let facts = super::policy::attributes(kind, value, status)?;
        let eligibility = if matches!(kind, "run" | "workflow" | "workflow_version") {
            let actor = value["owner"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Owner missing"))?;
            format!(
                "let $eligible=automation_owner_scope({},{},{},$t);$eligible==true;",
                q(&self.tenant),
                q(actor),
                native_now()
            )
        } else {
            String::new()
        };
        let mut configuration = self.configuration_policy(kind, id, value)?;
        if kind == "connection" {
            configuration
                .push_str("let $capacity=automation_connection_capacity($t);$capacity==true;");
        }
        storage::write(&self.state,&format!("{} {configuration}{eligibility} not {{$r isa automation_record,has auto_key {};}};select $t;insert $r isa automation_record,has auto_key {},has auto_tenant {},has auto_kind {},has auto_payload {},has auto_revision {},has auto_status {},has auto_when {when}{facts};{delivery}fetch {{'id':$r.auto_key}};",self.prefix(),q(&key),q(&key),q(&self.tenant),q(kind),q(&value.to_string()),q(&rev),q(status))).await?;
        Ok(Record {
            key,
            revision: rev,
            value: value.clone(),
            status: status.into(),
            when,
        })
    }
    pub async fn cas(
        &self,
        old: &Record,
        value: &Value,
        status: &str,
        when: i64,
    ) -> Result<Record> {
        self.cas_guarded(old, value, status, when, "").await
    }
    pub async fn claim(
        &self,
        old: &Record,
        value: &Value,
        status: &str,
        when: i64,
        actor: &str,
        connection: &Record,
    ) -> Result<Record> {
        let now = chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6f");
        let guard=format!("$u isa identity,has identity_id {};let $allowed=role_at_least($u,$t,\"owner\",{now});$allowed==true;$bound isa automation_record,has auto_tenant {},has auto_key {},has auto_revision {},has auto_status \"enabled\",has auto_enabled true,has auto_verified true;",q(actor),q(&self.tenant),q(&connection.key),q(&connection.revision));
        self.cas_guarded(old, value, status, when, &guard).await
    }
    pub(crate) async fn cas_guarded(
        &self,
        old: &Record,
        value: &Value,
        status: &str,
        when: i64,
        guard: &str,
    ) -> Result<Record> {
        self.update_record(old, value, status, when, guard, None)
            .await
    }
    pub async fn control(
        &self,
        old: &Record,
        value: &Value,
        status: &str,
        when: i64,
        actor: &str,
        action: &str,
    ) -> Result<Record> {
        ensure!(
            matches!(action, "approve" | "resolve" | "resume" | "cancel"),
            "Invalid run control"
        );
        self.for_owner(actor)?
            .update_record(old, value, status, when, "", Some(action))
            .await
    }
    async fn update_record(
        &self,
        old: &Record,
        value: &Value,
        status: &str,
        when: i64,
        guard: &str,
        control: Option<&str>,
    ) -> Result<Record> {
        let rev = uuid::Uuid::new_v4().to_string();
        let mut parts = old.key.rsplit('/');
        let id = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Invalid record key"))?;
        let kind = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Invalid record kind"))?;
        let delivery = crate::background::jobs::outbox(self, kind, id, &rev, status, when)?;
        let facts = super::policy::attributes(kind, value, status)?;
        let policy = if kind == "run" {
            let run: super::model::Run = serde_json::from_value(value.clone())?;
            let (count, effect) = run
                .attempt
                .as_ref()
                .map(|a| {
                    (
                        a.count,
                        serde_json::to_value(a.effect).expect("effect serializes"),
                    )
                })
                .unwrap_or((0, Value::String(String::new())));
            let decision = if let Some(action) = control {
                format!(
                    "let $policy=automation_control($r,{},{},{},{},{count},{},{});$policy==true;",
                    q(action),
                    chrono::Utc::now().timestamp(),
                    run.cursor,
                    q(status),
                    q(if action == "resolve" {
                        run.outputs
                            .get(
                                run.workflow
                                    .steps
                                    .get(run.cursor.saturating_sub(1))
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("Resolution cursor unavailable")
                                    })?
                                    .id(),
                            )
                            .and_then(|v| v["evidence"].as_str())
                            .unwrap_or("")
                    } else {
                        ""
                    }),
                    super::policy::approved(&run)
                )
            } else {
                format!("let $policy=automation_progress($r,$t,{},{},{},{},{count},{effect});$policy==true;",native_now(),chrono::Utc::now().timestamp(),q(status),run.cursor)
            };
            let history = if control == Some("approve") {
                String::new()
            } else {
                format!(
                    "$r has auto_approval_history {};",
                    q(&super::policy::approval_history(&run)?)
                )
            };
            format!(
                "$r has auto_owner {},has auto_steps {},has auto_definition {};{history}{decision}",
                q(&run.owner),
                run.workflow.steps.len(),
                q(&super::policy::definition(&run)?)
            )
        } else if matches!(kind, "schedule" | "trigger") {
            format!(
                "$r has auto_definition {};",
                q(&super::policy::origin_definition(kind, value)?)
            )
        } else if matches!(kind, "workflow" | "workflow_version") {
            format!(
                "let $eligible=automation_owner_scope({},{},{},$t);$eligible==true;",
                q(&self.tenant),
                q(value["owner"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Workflow owner missing"))?),
                native_now()
            )
        } else {
            String::new()
        };
        let configuration = self.configuration_policy(kind, id, value)?;
        let query=format!("{} {configuration}{guard} $r isa automation_record,has auto_tenant {},has auto_kind {},has auto_key {},has auto_revision {};{policy}select $t,$r;update $r has auto_payload {},has auto_revision {},has auto_status {},has auto_when {when}{facts};{delivery}fetch {{'id':$r.auto_key}};",self.prefix(),q(&self.tenant),q(kind),q(&old.key),q(&old.revision),q(&value.to_string()),q(&rev),q(status));
        if kind == "job" {
            storage::write_queue(&self.state, &query).await?;
        } else {
            storage::write(&self.state, &query).await?;
        }
        Ok(Record {
            key: old.key.clone(),
            revision: rev,
            value: value.clone(),
            status: status.into(),
            when,
        })
    }
    fn configuration_policy(&self, kind: &str, id: &str, value: &Value) -> Result<String> {
        Ok(match kind {
            "settings" if id=="billing"=> format!("let $configuration=bill_valid_reminder_days({});$configuration==true;",value["reminder_days"].as_i64().ok_or_else(||anyhow::anyhow!("Reminder period missing"))?),
            "schedule"=>format!("let $configuration=automation_schedule_configuration($t,{},{});$configuration==true;",q(&self.key("workflow",value["workflow"].as_str().ok_or_else(||anyhow::anyhow!("Workflow missing"))?)),value["every_seconds"].as_i64().ok_or_else(||anyhow::anyhow!("Schedule period missing"))?),
            _=>String::new()
        })
    }
    pub async fn owner(&self, actor: &str) -> Result<()> {
        let now = chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6f")
            .to_string();
        let rows=storage::read(&self.state,&format!("{} $u isa identity,has identity_id {};let $allowed=role_at_least($u,$t,\"owner\",{now});$allowed==true;fetch {{'id':$u.identity_id}};",self.prefix(),q(actor))).await?;
        ensure!(
            rows.len() == 1,
            dog_core::DogError::forbidden("Tenant owner permission required").into_anyhow()
        );
        Ok(())
    }
}
fn record(v: &Value) -> Result<Record> {
    Ok(Record {
        key: v["key"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing record key"))?
            .into(),
        revision: v["revision"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing record revision"))?
            .into(),
        value: serde_json::from_str(
            v["payload"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing record data"))?,
        )?,
        status: v["status"].as_str().unwrap_or("").into(),
        when: v["when"].as_i64().unwrap_or(0),
    })
}
pub async fn install(state: &TypeDBState) -> Result<()> {
    let schema = state
        .driver
        .databases()
        .get(&state.database)
        .await?
        .schema()
        .await?;
    if schema.contains("entity automation_record") {
        return Ok(());
    }
    let tx = state
        .driver
        .transaction(&state.database, typedb_driver::TransactionType::Schema)
        .await?;
    tx.query(SCHEMA).await?;
    tx.query(POLICY_SCHEMA).await?;
    tx.query(crate::database::BUSINESS_RULES).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn installed(state: &TypeDBState) -> Result<bool> {
    Ok(state
        .driver
        .databases()
        .get(&state.database)
        .await?
        .schema()
        .await?
        .contains("entity automation_record"))
}

pub(crate) fn native_now() -> String {
    chrono::Utc::now()
        .naive_utc()
        .format("%Y-%m-%dT%H:%M:%S%.6f")
        .to_string()
}
