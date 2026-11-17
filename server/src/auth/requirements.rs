//! Trusted per-service requirements, evaluated only after dog-auth verification.
use crate::BusinessParams;
use anyhow::{ensure, Result};
use serde::Deserialize;
use serde_json::Value;
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    #[serde(default)]
    pub operations: std::collections::BTreeMap<String, Requirement>,
    #[serde(default)]
    pub tenants: std::collections::BTreeMap<String, Requirement>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
    pub max_age_seconds: Option<i64>,
    pub acr: Option<String>,
}
impl Requirement {
    pub fn check(&self, verified: &Value) -> Result<()> {
        let denied = || {
            dog_core::DogError::forbidden("This operation requires stronger authentication")
                .into_anyhow()
        };
        if !self.providers.is_empty() {
            ensure!(
                verified["provider"]
                    .as_str()
                    .is_some_and(|p| self.providers.iter().any(|v| v == p)),
                denied()
            );
        }
        for method in &self.methods {
            ensure!(
                verified["amr"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == method)),
                denied()
            );
        }
        if let Some(age) = self.max_age_seconds {
            ensure!(
                age > 0
                    && verified["auth_time"]
                        .as_i64()
                        .is_some_and(|v| v <= crate::clock::unix_seconds() + 30
                            && crate::clock::unix_seconds() - v <= age),
                denied()
            );
        }
        if let Some(acr) = &self.acr {
            ensure!(verified["acr"] == *acr, denied());
        }
        Ok(())
    }
}
pub struct Require(pub Requirement);
#[async_trait::async_trait]
impl dog_core::DogBeforeHook<Value, BusinessParams> for Require {
    async fn run(&self, ctx: &mut dog_core::HookContext<Value, BusinessParams>) -> Result<()> {
        ensure!(ctx.params.authenticated, "Authentication must run first");
        let result = ctx
            .params
            .auth_result
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing verified authentication"))?;
        self.0.check(&result["authentication"])?;
        let operation = match ctx.method {
            dog_core::ServiceMethodKind::Find => "find",
            dog_core::ServiceMethodKind::Get => "get",
            dog_core::ServiceMethodKind::Create => "create",
            dog_core::ServiceMethodKind::Update => "update",
            dog_core::ServiceMethodKind::Patch => "patch",
            dog_core::ServiceMethodKind::Remove => "remove",
            dog_core::ServiceMethodKind::Custom(name) => name,
        };
        if let Some(rule) = self.0.operations.get(operation) {
            rule.check(&result["authentication"])?;
        }
        if let Some(rule) = self.0.tenants.get(&ctx.tenant.tenant_id.0) {
            rule.check(&result["authentication"])?;
            if let Some(rule) = rule.operations.get(operation) {
                rule.check(&result["authentication"])?;
            }
        }
        Ok(())
    }
}
pub fn configure(
    builder: &mut dog_core::DogAppBuilder<Value, BusinessParams>,
) -> Result<Vec<String>> {
    let rules: std::collections::BTreeMap<String, Requirement> = serde_json::from_str(
        &std::env::var("AUTH_SERVICE_REQUIREMENTS_JSON").unwrap_or_else(|_| "{}".into()),
    )?;
    let mut configured = Vec::new();
    for (service, rule) in rules {
        ensure!(
            !service.is_empty()
                && service.len() <= 200
                && service
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_')),
            "Invalid authentication service name"
        );
        configured.push(service.clone());
        ensure!(
            rule.max_age_seconds.is_none_or(|v| v > 0),
            "Authentication age must be positive"
        );
        ensure!(
            service != "authentication",
            "Authentication endpoint cannot require itself"
        );
        builder.service_hooks(&service, |h| {
            if service == "data" {
                h.before(
                    dog_core::ServiceMethodKind::Custom("read"),
                    std::sync::Arc::new(Require(rule.clone())),
                );
                h.before(
                    dog_core::ServiceMethodKind::Custom("write"),
                    std::sync::Arc::new(Require(rule)),
                );
            } else {
                h.before_all(std::sync::Arc::new(Require(rule)));
            }
        });
    }
    Ok(configured)
}
