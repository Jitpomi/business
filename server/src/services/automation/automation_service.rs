//! The verified tenant context is the only source of connection ownership.
use crate::{
    automation::{
        engine::Engine,
        model::{Connection, Schedule, Workflow},
        store::Store,
    },
    services::BusinessParams,
};
use anyhow::Result;
use dog_core::{DogService, ServiceCapabilities, TenantContext};
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    List {
        kind: String,
        after: Option<String>,
        #[serde(default = "super::automation_schema::limit")]
        limit: usize,
    },
    Inspect {
        kind: String,
        id: String,
    },
    Trigger {
        id: String,
        workflow: String,
        enabled: bool,
    },
    ConfigureBilling {
        settings: crate::automation::managed::Settings,
    },
    Connect {
        id: String,
        connector: String,
        config: Value,
        #[serde(default)]
        secrets: Value,
    },
    Verify {
        id: String,
    },
    Disconnect {
        id: String,
    },
    Publish {
        workflow: Workflow,
    },
    Run {
        workflow: String,
        key: String,
        #[serde(default)]
        input: Value,
    },
    Schedule {
        schedule: Schedule,
    },
    Approve {
        id: String,
    },
    Cancel {
        id: String,
    },
    Resume {
        id: String,
    },
    Resolve {
        id: String,
        evidence: String,
        output: Value,
    },
}
pub struct AutomationService {
    pub template: std::sync::Arc<Engine>,
}
impl AutomationService {
    fn engine(&self, tenant: &str, actor: &str) -> Result<Engine> {
        Ok(Engine {
            store: Store::new(self.template.store.state.clone(), tenant)?.for_owner(actor)?,
            catalog: self.template.catalog.clone(),
            vault: self.template.vault.clone(),
        })
    }
}
#[async_trait::async_trait]
impl DogService<Value, BusinessParams> for AutomationService {
    fn capabilities(&self) -> ServiceCapabilities {
        super::automation_shared::capabilities()
    }
    async fn find(&self, ctx: &TenantContext, params: BusinessParams) -> Result<Vec<Value>> {
        let actor = super::automation_schema::actor(ctx, &params)?;
        let e = self.engine(&ctx.tenant_id.0, &actor)?;
        e.store.owner(&actor).await?;
        let mut connections = vec![];
        for r in e.store.list("connection", 100).await? {
            let c: Connection = serde_json::from_value(r.value)?;
            connections.push(c.public());
        }
        let mut out = json!({"catalog":e.catalog.describe(),"connections":connections});
        out["job"] = json!(e.store.summaries("job", 100).await?);
        out["run"] = json!(e.store.summaries("run", 100).await?);
        out["workflow_version"] = json!(e.store.summaries("workflow_version", 100).await?);
        out["billing_settings"] = e
            .store
            .get("settings", "billing")
            .await?
            .map(|r| r.value)
            .unwrap_or(Value::Null);
        for kind in ["workflow", "schedule"] {
            out[kind] = json!(e
                .store
                .list(kind, 100)
                .await?
                .into_iter()
                .map(
                    |r| json!({"data":r.value,"status":r.status,"revision":r.revision,"due":r.when})
                )
                .collect::<Vec<_>>());
        }
        Ok(vec![out])
    }
    async fn create(
        &self,
        ctx: &TenantContext,
        data: Value,
        params: BusinessParams,
    ) -> Result<Value> {
        let actor = super::automation_schema::actor(ctx, &params)?;
        let e = self.engine(&ctx.tenant_id.0, &actor)?;
        e.store.owner(&actor).await?;
        match serde_json::from_value::<Command>(data).map_err(|_| {
            dog_core::DogError::bad_request("Unsupported or malformed automation request")
                .into_anyhow()
        })? {
            Command::List { kind, after, limit } => {
                super::automation_schema::kind(&kind)?;
                let rows = e.store.page(&kind, after.as_deref(), limit).await?;
                let next = if rows.len() == limit {
                    rows.last().map(|r| r["key"].clone())
                } else {
                    None
                };
                Ok(json!({"records":rows,"next":next}))
            }
            Command::Inspect { kind, id } => {
                super::automation_schema::kind(&kind)?;
                crate::automation::model::identifier(&id)?;
                let record = e
                    .store
                    .get(&kind, &id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Record unavailable"))?;
                if kind == "connection" {
                    let c: Connection = serde_json::from_value(record.value)?;
                    Ok(c.public())
                } else if kind == "job" {
                    // Lease ownership remains private even to tenant owners.
                    let job: dog_queue::JobRecord =
                        serde_json::from_value(record.value["record"].clone())?;
                    Ok(
                        json!({"data":job,"status":record.status,"revision":record.revision,"due":record.when}),
                    )
                } else if kind == "trigger" {
                    Ok(
                        json!({"id":record.value["id"],"workflow":record.value["workflow"],"enabled":record.value["enabled"]}),
                    )
                } else {
                    Ok(
                        json!({"data":record.value,"status":record.status,"revision":record.revision,"due":record.when}),
                    )
                }
            }
            Command::Trigger {
                id,
                workflow,
                enabled,
            } => e.trigger(&id, &workflow, enabled).await,
            Command::ConfigureBilling { settings } => e.configure_billing(settings).await,
            Command::Connect {
                id,
                connector,
                config,
                secrets,
            } => e.connect(&id, &connector, config, secrets).await,
            Command::Verify { id } => e.verify_connection(&id).await,
            Command::Disconnect { id } => e.disconnect(&id).await,
            Command::Publish { workflow } => e.publish(workflow, &actor).await,
            Command::Run {
                workflow,
                key,
                input,
            } => e.enqueue(&workflow, &key, input).await,
            Command::Schedule { schedule } => e.schedule(schedule).await,
            Command::Approve { id } => e.approve(&id, &actor).await,
            Command::Cancel { id } => e.cancel(&id).await,
            Command::Resume { id } => e.resume(&id).await,
            Command::Resolve {
                id,
                evidence,
                output,
            } => e.resolve(&id, &actor, &evidence, output).await,
        }
    }
}
