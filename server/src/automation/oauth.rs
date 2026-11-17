//! Plugin-owned OAuth; native one-time claims and vault-owned credentials.
use super::{
    engine::{now, Engine},
    model::Connection,
    store::q,
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
pub struct Grant {
    pub config: Value,
    pub secrets: Value,
}
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn describe(&self) -> Value;
    /// Exact registered redirect and S256 PKCE where supported.
    fn authorize(&self, state: &str, verifier: &str) -> Result<String>;
    async fn exchange(&self, code: &str, verifier: &str) -> Result<Grant>;
}
fn bad(message: &str) -> anyhow::Error {
    dog_core::DogError::bad_request(message).into_anyhow()
}
fn conflict() -> anyhow::Error {
    dog_core::DogError::conflict("This request expired, was used, or belongs to another account. Start again from Connections.").into_anyhow()
}
fn digest(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
fn random() -> String {
    URL_SAFE_NO_PAD.encode(
        [
            uuid::Uuid::new_v4().as_bytes().as_slice(),
            uuid::Uuid::new_v4().as_bytes().as_slice(),
        ]
        .concat(),
    )
}
impl Engine {
    pub async fn oauth_begin(&self, id: &str, connector: &str) -> Result<Value> {
        super::model::identifier(id).map_err(|_| bad("Choose a valid connection name"))?;
        let actor = self.store.request_actor()?;
        self.store.owner(actor).await?;
        ensure!(
            self.store.get("connection", id).await?.is_none(),
            dog_core::DogError::conflict("That connection name is already in use").into_anyhow()
        );
        let plugin = self.catalog.get(connector)?;
        let provider = plugin
            .oauth()
            .ok_or_else(|| bad("This plugin does not offer OAuth onboarding"))?;
        let state = random();
        let verifier = random();
        let key = digest(&state);
        let url = provider.authorize(&state, &verifier)?;
        let secret = self
            .vault
            .seal(
                &format!("{}/oauth/{key}", self.store.tenant),
                &json!({"verifier":verifier}),
            )
            .await?;
        self.store
            .insert(
                "oauth",
                &key,
                &json!({"owner":actor,"id":id,"connector":connector,"secret_ref":secret}),
                "pending",
                now() + 600,
            )
            .await?;
        Ok(json!({"authorization_url":url,"state":state,"expires_in":600}))
    }
    pub async fn oauth_finish(
        &self,
        state: &str,
        code: Option<&str>,
        denied: bool,
    ) -> Result<Value> {
        ensure!(
            (40..=128).contains(&state.len())
                && state
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            bad("Invalid connection response")
        );
        ensure!(
            denied
                || code.is_some_and(|c| !c.is_empty()
                    && c.len() <= 4096
                    && !c.chars().any(char::is_control)),
            bad("Missing authorization code")
        );
        let actor = self.store.request_actor()?;
        let key = digest(state);
        let r = self.store.get("oauth", &key).await?.ok_or_else(conflict)?;
        let claim = format!(
            "let $claim=automation_oauth_claim($r,$t,{},{},{});$claim==true;",
            q(actor),
            super::store::native_now(),
            now()
        );
        // Never retry a code exchange after an uncertain acknowledgement.
        let claimed = self
            .store
            .cas_guarded(
                &r,
                &r.value,
                if denied { "denied" } else { "exchanging" },
                r.when,
                &claim,
            )
            .await?;
        if denied {
            return Ok(json!({"cancelled":true}));
        }
        let connector = r.value["connector"].as_str().ok_or_else(conflict)?;
        let id = r.value["id"].as_str().ok_or_else(conflict)?;
        let plugin = self.catalog.get(connector)?;
        let provider = plugin.oauth().ok_or_else(conflict)?;
        let pending = self
            .vault
            .open(
                &format!("{}/oauth/{key}", self.store.tenant),
                r.value["secret_ref"].as_str().ok_or_else(conflict)?,
            )
            .await?;
        let grant = provider
            .exchange(
                code.unwrap(),
                pending["verifier"].as_str().ok_or_else(conflict)?,
            )
            .await?;
        plugin.validate(&grant.config, &grant.secrets)?;
        ensure!(
            super::connectors::redact(grant.config.clone(), &grant.secrets) == grant.config,
            "OAuth credential in public configuration"
        );
        let revision = uuid::Uuid::new_v4().to_string();
        let secret_ref = self
            .vault
            .seal(
                &format!("{}/{id}/{revision}", self.store.tenant),
                &grant.secrets,
            )
            .await?;
        let c = Connection {
            id: id.into(),
            connector: connector.into(),
            revision,
            config: grant.config,
            secret_ref,
            enabled: true,
            verified: false,
            verification: None,
        };
        let facts = super::policy::attributes("connection", &serde_json::to_value(&c)?, "enabled")?;
        // Connection and completed handoff commit atomically, with native owner,
        // expiry, capacity and revision checks after the asynchronous exchange.
        let query=format!("{} $r isa automation_record,has auto_key {},has auto_revision {};let $finish=automation_oauth_finish($r,$t,{},{},{});$finish==true;not {{$existing isa automation_record,has auto_key {};}};select $t,$r;insert $c isa automation_record,has auto_key {},has auto_tenant {},has auto_kind \"connection\",has auto_payload {},has auto_revision {},has auto_status \"enabled\",has auto_when 0{facts};update $r has auto_status \"completed\",has auto_revision {};fetch {{'id':$c.auto_key}};",self.store.prefix(),q(&r.key),q(&claimed.revision),q(actor),super::store::native_now(),now(),q(&self.store.key("connection",id)),q(&self.store.key("connection",id)),q(&self.store.tenant),q(&serde_json::to_value(&c)?.to_string()),q(&uuid::Uuid::new_v4().to_string()),q(&uuid::Uuid::new_v4().to_string()));
        crate::storage::write(&self.store.state, &query).await?;
        Ok(c.public())
    }
}
