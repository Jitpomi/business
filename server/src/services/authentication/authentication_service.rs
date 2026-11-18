//! Authentication transport delegates proofs and account lifecycle to registered plugins.
use crate::{auth::Registry, services::BusinessParams, typedb::TypeDBState};
use anyhow::{ensure, Result};
use dog_core::{DogService, ServiceCapabilities, ServiceMethodKind, TenantContext};
use serde_json::{json, Value};
use std::sync::Arc;
pub type AuthService = dog_auth::AuthServiceAdapter<BusinessParams>;
pub struct PluginAuthentication {
    pub auth: Arc<AuthService>,
    pub plugins: Arc<Registry>,
    pub state: Arc<TypeDBState>,
    pub financial: Arc<crate::finance::TenantRegistry>,
}
#[async_trait::async_trait]
impl DogService<Value, BusinessParams> for PluginAuthentication {
    fn capabilities(&self) -> ServiceCapabilities {
        ServiceCapabilities::from_methods(vec![
            ServiceMethodKind::Find,
            ServiceMethodKind::Create,
            ServiceMethodKind::Remove,
            ServiceMethodKind::Custom("account"),
        ])
    }
    async fn find(&self, _: &TenantContext, _: BusinessParams) -> Result<Vec<Value>> {
        Ok(vec![self.plugins.public()])
    }
    async fn create(
        &self,
        ctx: &TenantContext,
        data: Value,
        params: BusinessParams,
    ) -> Result<Value> {
        self.auth.create(ctx, data, params).await
    }
    async fn remove(
        &self,
        ctx: &TenantContext,
        id: Option<&str>,
        params: BusinessParams,
    ) -> Result<Value> {
        let token = id
            .map(str::to_owned)
            .or_else(|| dog_auth::core::extract_bearer_token(&params.headers))
            .ok_or_else(|| dog_core::DogError::not_authenticated("Missing token").into_anyhow())?;
        if let Ok(verified) = self.plugins.verify(&token).await {
            let plugin = self.plugins.plugin(&verified.provider)?;
            return plugin.account("logout", &json!({})).await;
        }
        self.auth.remove(ctx, id, params).await
    }
    async fn custom(
        &self,
        ctx: &TenantContext,
        method: &str,
        data: Option<Value>,
        params: BusinessParams,
    ) -> Result<Value> {
        ensure!(
            method == "account",
            dog_core::DogError::method_not_allowed("Unsupported authentication operation")
                .into_anyhow()
        );
        let input = data.ok_or_else(|| {
            dog_core::DogError::bad_request("Missing account request").into_anyhow()
        })?;
        if input["operation"] == "checkout_enrolment" {
            return self.checkout_enrolment(&input).await;
        }
        if input["operation"] == "accept_invitation" {
            return self.accept_invitation(&input).await;
        }
        if input["operation"] == "invitation" {
            return self.invitation(ctx, &input, params).await;
        }
        let operation = input["operation"].as_str().unwrap_or("");
        ensure!(
            matches!(
                operation,
                "link" | "login" | "signup" | "manage" | "recover" | "logout"
            ),
            dog_core::DogError::bad_request("Unsupported authentication operation").into_anyhow()
        );
        let plugin = self
            .plugins
            .plugin(input["plugin"].as_str().unwrap_or(""))
            .map_err(|_| {
                dog_core::DogError::bad_request("Unknown authentication plugin").into_anyhow()
            })?;
        let operation = input["operation"].as_str().unwrap_or("");
        if operation != "link" {
            return plugin.account(operation, &input).await;
        }
        // Linking requires proof of the current account AND the new external
        // account. Matching email addresses never establish identity ownership.
        let current = dog_auth::core::extract_bearer_token(&params.headers).ok_or_else(|| {
            dog_core::DogError::not_authenticated("Authenticate the current account first")
                .into_anyhow()
        })?;
        let user = self
            .auth
            .create(ctx, json!({"strategy":"jwt","accessToken":current}), params)
            .await?;
        crate::auth::requirements::Requirement {
            methods: vec!["mfa".into()],
            max_age_seconds: Some(300),
            ..Default::default()
        }
        .check(&user["authentication"])?;
        let person = user["user"]["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing verified person"))?;
        let proof = input["accessToken"].as_str().ok_or_else(|| {
            dog_core::DogError::bad_request("Prove control of the account being linked")
                .into_anyhow()
        })?;
        let verified = self
            .plugins
            .verify_plugin(plugin.id(), proof)
            .await
            .map_err(|_| {
                dog_core::DogError::not_authenticated("Invalid external account proof")
                    .into_anyhow()
            })?;
        let key = crate::auth::binding_key(&verified.issuer, &verified.subject);
        let q = |s: &str| serde_json::to_string(s).unwrap();
        let existing=crate::storage::read(&self.state,&format!("match $l isa external_identity,has auth_binding_key {},links(identity:$p);fetch {{'id':$p.identity_id}};",q(&key))).await?;
        if let Some(old) = existing.first() {
            ensure!(
                existing.len() == 1 && old["id"] == person,
                dog_core::DogError::conflict(
                    "External account is already linked to another person"
                )
                .into_anyhow()
            );
            return Ok(json!({"linked":true}));
        }
        let query=format!("match $t isa tenant,has tenant_id {};$p isa person,has identity_id {};let $active=active_member($p,$t,{});$active==true;not {{$l isa external_identity,has auth_binding_key {};}};select $t,$p;distinct;insert $l isa external_identity,links(identity:$p),has auth_binding_key {},has auth_issuer {},has auth_subject {};fetch {{'linked':true}};",q(&ctx.tenant_id.0),q(person),crate::clock::native_utc(),q(&key),q(&key),q(&verified.issuer),q(&verified.subject));
        crate::storage::write(&self.state, &query).await
    }
}
