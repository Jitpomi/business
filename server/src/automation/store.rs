//! Tenant-qualified durable records and compare-and-swap claims.
use crate::{storage, typedb::TypeDBState};
use anyhow::{ensure, Result};
use serde_json::Value;
use std::sync::Arc;
pub const SCHEMA: &str = include_str!("schema.tql");
pub fn q(v: &str) -> String {
    serde_json::to_string(v).unwrap()
}
#[derive(Clone)]
pub struct Store {
    pub state: Arc<TypeDBState>,
    pub tenant: String,
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
        })
    }
    pub fn prefix(&self) -> String {
        format!("match $t isa tenant,has tenant_id {};", q(&self.tenant))
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
        storage::write(&self.state,&format!("{} not {{$r isa automation_record,has auto_key {};}};select $t;insert $r isa automation_record,has auto_key {},has auto_tenant {},has auto_kind {},has auto_payload {},has auto_revision {},has auto_status {},has auto_when {when};fetch {{'id':$r.auto_key}};",self.prefix(),q(&key),q(&key),q(&self.tenant),q(kind),q(&value.to_string()),q(&rev),q(status))).await?;
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
        let guard=format!("$u isa identity,has identity_id {};let $allowed=role_at_least($u,$t,\"owner\",{now});$allowed==true;$bound isa automation_record,has auto_tenant {},has auto_key {},has auto_revision {},has auto_status \"enabled\";",q(actor),q(&self.tenant),q(&connection.key),q(&connection.revision));
        self.cas_guarded(old, value, status, when, &guard).await
    }
    async fn cas_guarded(
        &self,
        old: &Record,
        value: &Value,
        status: &str,
        when: i64,
        guard: &str,
    ) -> Result<Record> {
        let rev = uuid::Uuid::new_v4().to_string();
        storage::write(&self.state,&format!("{} {guard} $r isa automation_record,has auto_tenant {},has auto_key {},has auto_revision {};select $t,$r;update $r has auto_payload {},has auto_revision {},has auto_status {},has auto_when {when};fetch {{'id':$r.auto_key}};",self.prefix(),q(&self.tenant),q(&old.key),q(&old.revision),q(&value.to_string()),q(&rev),q(status))).await?;
        Ok(Record {
            key: old.key.clone(),
            revision: rev,
            value: value.clone(),
            status: status.into(),
            when,
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
