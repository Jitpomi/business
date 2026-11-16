//! dog-auth orchestration. Providers own credentials; TypeDB owns identity links and RBAC.
pub mod accounts;
pub mod plugins;
pub mod requirements;
pub mod token_store;
use crate::{services::BusinessParams, typedb::TypeDBState};
use anyhow::{ensure, Result};
use async_trait::async_trait;
use dog_auth::{
    core::{
        AuthenticationBase, AuthenticationParams, AuthenticationRequest, AuthenticationResult,
        AuthenticationStrategy,
    },
    AuthOptions, AuthStrategy, AuthenticationService, JwtStrategy,
};
use dog_auth_local::{LocalEntityResolver, LocalStrategy};
use dog_core::{DogAppBuilder, DogError, HookContext};
use dog_typedb::TypeDBAdapter;
use serde_json::{json, Value};
use std::sync::Arc;
#[derive(Clone, serde::Serialize)]
pub struct Verified {
    pub provider: String,
    pub issuer: String,
    pub subject: String,
    pub expires: i64,
    pub auth_time: Option<i64>,
    pub amr: Vec<String>,
    pub acr: Option<String>,
    pub email: Option<String>,
    pub email_verified: bool,
}
impl Verified {
    pub fn from_claims(provider: &str, c: &Value) -> Result<Self> {
        let get = |k: &str| {
            c[k].as_str()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Missing verified identity claim"))
        };
        let subject = get("sub")?.to_owned();
        ensure!(subject.len() <= 255, "Subject exceeds limit");
        let amr = match c.get("amr") {
            None => Vec::new(),
            Some(Value::Array(v)) => {
                ensure!(v.len() <= 16, "Too many authentication methods");
                v.iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| anyhow::anyhow!("Invalid authentication method"))
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            _ => anyhow::bail!("Invalid authentication methods"),
        };
        let now = chrono::Utc::now().timestamp();
        let auth_time = c
            .get("auth_time")
            .map(|v| {
                v.as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Invalid authentication time"))
            })
            .transpose()?;
        ensure!(
            auth_time.is_none_or(|v| v <= now + 30),
            "Authentication time is in the future"
        );
        Ok(Self {
            provider: provider.into(),
            issuer: get("iss")?.into(),
            subject,
            expires: c["exp"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing expiry"))?,
            auth_time,
            amr,
            acr: c["acr"].as_str().map(str::to_owned),
            email: c["email"].as_str().map(str::to_owned),
            email_verified: c["email_verified"] == true,
        })
    }
}
#[async_trait]
pub trait AuthenticationPlugin: Send + Sync {
    fn id(&self) -> &str;
    fn issuer(&self) -> &str;
    async fn verify(&self, token: &str) -> Result<Verified>;
    fn account_url(&self) -> Option<&str> {
        None
    }
    async fn account(&self, operation: &str, _input: &Value) -> Result<Value> {
        ensure!(
            matches!(
                operation,
                "login" | "signup" | "manage" | "recover" | "logout"
            ),
            dog_core::DogError::method_not_allowed("Unsupported provider account operation")
                .into_anyhow()
        );
        let url = self.account_url().ok_or_else(|| {
            dog_core::DogError::method_not_allowed("Use this provider's SDK or hosted account flow")
                .into_anyhow()
        })?;
        Ok(json!({"flow":"provider","operation":operation,"account_url":url,"completed":false}))
    }
}
#[async_trait]
impl AuthenticationPlugin for plugins::oidc::Oidc {
    fn id(&self) -> &str {
        &self.config.id
    }
    fn issuer(&self) -> &str {
        &self.config.issuer
    }
    async fn verify(&self, token: &str) -> Result<Verified> {
        self.verify(token).await
    }
    fn account_url(&self) -> Option<&str> {
        self.config.account_url.as_deref()
    }
}
#[derive(Clone, Default)]
pub struct Registry {
    providers: Vec<Arc<dyn AuthenticationPlugin>>,
    pub local: Option<Arc<dyn accounts::LocalAccounts>>,
}
impl Registry {
    pub fn public(&self) -> Value {
        json!({"providers":self.providers.iter().map(|p|json!({"id":p.id(),"issuer":p.issuer(),"account_url":p.account_url()})).collect::<Vec<_>>(),"local_enabled":self.local.is_some()})
    }

    pub fn register(&mut self, p: Arc<dyn AuthenticationPlugin>) -> Result<()> {
        ensure!(
            p.id() != "local"
                && p.id() != "jwt"
                && !p.id().is_empty()
                && p.id().len() <= 64
                && p.id()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "Invalid authentication plugin ID"
        );
        ensure!(
            !self.providers.iter().any(|old| old.id() == p.id()),
            "Duplicate authentication plugin"
        );
        self.providers.push(p);
        Ok(())
    }
    pub fn configured(database: &str) -> Result<Self> {
        let mut registry = Self::default();
        for config in serde_json::from_str::<Vec<plugins::oidc::Config>>(
            &std::env::var("AUTH_OIDC_PROVIDERS_JSON").unwrap_or_else(|_| "[]".into()),
        )? {
            registry.register(Arc::new(plugins::oidc::Oidc::new(config)?))?;
        }
        if std::env::var("AUTH_LOCAL_ENABLED").as_deref() == Ok("true") {
            registry.local = Some(Arc::new(plugins::local::Local::configured(database)?));
        }
        ensure!(
            registry.local.is_some() || !registry.providers.is_empty(),
            "Configure at least one authentication plugin; no implicit local fallback"
        );
        Ok(registry)
    }
    pub fn plugin(&self, id: &str) -> Result<Arc<dyn AuthenticationPlugin>> {
        self.providers
            .iter()
            .find(|p| p.id() == id)
            .cloned()
            .ok_or_else(|| {
                dog_core::DogError::bad_request("Authentication plugin not configured")
                    .into_anyhow()
            })
    }
    pub fn local(&self) -> Result<Arc<dyn accounts::LocalAccounts>> {
        self.local.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Local account management is not enabled; use the configured identity provider"
            )
        })
    }
    pub async fn verify_plugin(&self, id: &str, token: &str) -> Result<Verified> {
        ensure!(token.len() <= 32768, "Authentication proof exceeds limit");
        let plugin = self.plugin(id)?;
        let verified = plugin.verify(token).await?;
        ensure!(
            verified.provider == plugin.id()
                && verified.issuer == plugin.issuer()
                && !verified.subject.is_empty()
                && verified.subject.len() <= 255
                && verified.expires > chrono::Utc::now().timestamp(),
            "Plugin returned inconsistent or expired identity"
        );
        Ok(verified)
    }
    pub async fn verify(&self, token: &str) -> Result<Verified> {
        // Unverified issuer is only a lookup hint into the explicitly trusted registry.
        use base64::Engine;
        ensure!(token.len() <= 32768, "Token exceeds limit");
        let raw = token
            .split('.')
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Invalid token"))?;
        let claims: Value =
            serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw)?)?;
        let issuer = claims["iss"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing issuer"))?;
        for provider in self.providers.iter().filter(|p| p.issuer() == issuer) {
            if let Ok(v) = self.verify_plugin(provider.id(), token).await {
                return Ok(v);
            }
        }
        anyhow::bail!("No trusted authentication plugin accepted the token")
    }
}
#[derive(Clone)]
struct Resolver {
    adapter: Arc<TypeDBAdapter>,
    registry: Arc<Registry>,
    discover: bool,
}
impl Resolver {
    async fn identity(&self, issuer: &str, subject: &str, tenant: &str) -> Result<Option<Value>> {
        let q = |v: &str| serde_json::to_string(v).unwrap();
        let scope = if tenant == "account" {
            String::new()
        } else {
            format!("$t has tenant_id {};", q(tenant))
        };
        let query=format!("match $i isa identity,has identity_id $id;$link isa external_identity,links(identity:$i),has auth_binding_key {},has auth_issuer {},has auth_subject {};$t isa tenant,has tenant_id $tenant;{scope}let $active=active_member($i,$t,{});$active==true;select $i,$t,$id,$tenant;distinct;sort $tenant;limit 1;fetch {{'id':$id,'tenant_id':$tenant}};",q(&binding_key(issuer,subject)),q(issuer),q(subject),now());
        let response = self.adapter.read(json!({"query":query})).await?;
        let rows = response["ok"]["answers"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid identity response"))?;
        Ok(rows.first().map(|r| r["data"].clone()))
    }
}
#[async_trait]
impl LocalEntityResolver<BusinessParams> for Resolver {
    async fn resolve_entity(
        &self,
        email: &str,
        ctx: &mut HookContext<Value, BusinessParams>,
    ) -> Result<Option<Value>> {
        let Some(account) = self
            .registry
            .local()?
            .lookup(&email.trim().to_ascii_lowercase(), true)?
        else {
            return Ok(None);
        };
        let tenant = if self.discover {
            "account"
        } else {
            &ctx.tenant.tenant_id.0
        };
        let Some(mut person) = self
            .identity(
                "urn:jitpomi:local",
                account["subject"].as_str().unwrap(),
                tenant,
            )
            .await?
        else {
            return Ok(None);
        };
        person["password"] = account["password"].clone();
        person["email"] = account["email"].clone();
        person["credential_id"] = account["subject"].clone();
        person["credential_version"] = account["version"].clone();
        Ok(Some(person))
    }
}
struct Local(LocalStrategy<BusinessParams>);
#[async_trait]
impl AuthenticationStrategy<BusinessParams> for Local {
    async fn authenticate(
        &self,
        request: &AuthenticationRequest,
        params: &AuthenticationParams,
        ctx: &mut HookContext<Value, BusinessParams>,
        auth: &AuthenticationBase<BusinessParams>,
    ) -> Result<AuthenticationResult> {
        let mut result = self.0.authenticate(request, params, ctx, auth).await?;
        let at = chrono::Utc::now().timestamp();
        result["accessToken"]=json!(auth.create_access_token(json!({"sub":result["user"]["id"],"credential_id":result["user"]["credential_id"],"credential_version":result["user"]["credential_version"],"auth_time":at,"amr":["pwd"]}),None).await?);
        Ok(result)
    }
}
struct Jwt {
    inner: JwtStrategy<BusinessParams>,
    resolver: Resolver,
}
#[async_trait]
impl AuthenticationStrategy<BusinessParams> for Jwt {
    async fn authenticate(
        &self,
        request: &AuthenticationRequest,
        params: &AuthenticationParams,
        ctx: &mut HookContext<Value, BusinessParams>,
        auth: &AuthenticationBase<BusinessParams>,
    ) -> Result<AuthenticationResult> {
        let token = request
            .data
            .get("accessToken")
            .and_then(Value::as_str)
            .ok_or_else(|| DogError::not_authenticated("Missing token").into_anyhow())?;
        let hint = params
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-auth-provider"))
            .map(|(_, v)| v.as_str())
            .or_else(|| request.data.get("plugin").and_then(Value::as_str));
        let external = if let Some(id) = hint {
            self.resolver.registry.verify_plugin(id, token).await
        } else {
            self.resolver.registry.verify(token).await
        };
        if let Ok(verified) = external {
            let user = self
                .resolver
                .identity(&verified.issuer, &verified.subject, &ctx.tenant.tenant_id.0)
                .await?
                .ok_or_else(|| {
                    DogError::not_authenticated("Identity or tenant membership unavailable")
                        .into_anyhow()
                })?;
            return Ok(json!({"accessToken":token,"user":user,"authentication":verified}));
        }
        // An external-provider failure never enables local authentication. Only
        // locally signed platform sessions can pass this separate verifier.
        if hint.is_some() {
            return Err(DogError::not_authenticated(
                "Selected authentication plugin rejected the proof",
            )
            .into_anyhow());
        }
        let local = self.resolver.registry.local().map_err(|_| {
            DogError::not_authenticated("Untrusted authentication token").into_anyhow()
        })?;
        let mut result = self.inner.authenticate(request, params, ctx, auth).await?;
        let payload = &result["payload"];
        let subject = payload["credential_id"]
            .as_str()
            .ok_or_else(|| DogError::not_authenticated("Sign in again").into_anyhow())?;
        let account = local
            .lookup(subject, false)?
            .ok_or_else(|| DogError::not_authenticated("Account unavailable").into_anyhow())?;
        ensure!(
            account["version"] == payload["credential_version"],
            DogError::not_authenticated("Credentials changed").into_anyhow()
        );
        let person = self
            .resolver
            .identity("urn:jitpomi:local", subject, &ctx.tenant.tenant_id.0)
            .await?
            .ok_or_else(|| DogError::not_authenticated("Membership unavailable").into_anyhow())?;
        ensure!(
            person["id"] == payload["sub"],
            DogError::not_authenticated("Identity changed").into_anyhow()
        );
        result["authentication"] = json!(Verified {
            provider: "local".into(),
            issuer: "urn:jitpomi:local".into(),
            subject: subject.into(),
            expires: payload["exp"].as_i64().unwrap_or(0),
            auth_time: payload["auth_time"].as_i64(),
            amr: vec!["pwd".into()],
            acr: None,
            email: None,
            email_verified: false
        });
        result["user"] = person;
        Ok(result)
    }
}
pub fn strategies(
    builder: &mut DogAppBuilder<Value, BusinessParams>,
    state: Arc<TypeDBState>,
) -> Result<Arc<dog_auth::AuthServiceAdapter<BusinessParams>>> {
    let registry: Arc<Registry> = match builder.get("authentication.plugins") {
        Some(v) => v,
        None => Arc::new(Registry::configured(&state.database)?),
    };
    builder.set("authentication.plugins", registry.clone());
    let mut opts = AuthOptions {
        strategies: vec![AuthStrategy::Jwt],
        ..Default::default()
    };
    if registry.local.is_some() {
        opts.strategies.extend([
            AuthStrategy::Custom("local".into()),
            AuthStrategy::Custom("account".into()),
        ]);
    }
    opts.jwt.secret = builder.get("auth.jwt.secret");
    opts.entity = Some("user".into());
    let mut auth = AuthenticationService::builder(builder, Some(opts))?;
    if let Some(local) = registry.local.clone() {
        auth = auth.with_token_store(Arc::new(token_store::LocalTokenStore::new(local)));
    }
    let resolver = Resolver {
        adapter: Arc::new(TypeDBAdapter::new(state)),
        registry,
        discover: false,
    };
    if resolver.registry.local.is_some() {
        for (name, discover) in [("local", false), ("account", true)] {
            auth.register(
                name,
                Arc::new(Local(LocalStrategy::new().with_entity_resolver(Arc::new(
                    Resolver {
                        discover,
                        ..resolver.clone()
                    },
                )))),
            );
        }
    }
    auth.register(
        "jwt",
        Arc::new(Jwt {
            inner: JwtStrategy::new(),
            resolver,
        }),
    );
    let service = Arc::new(AuthenticationService::new(Arc::new(auth.build())));
    Ok(AuthenticationService::install(builder, service))
}
fn now() -> String {
    chrono::Utc::now()
        .naive_utc()
        .format("%Y-%m-%dT%H:%M:%S%.6f")
        .to_string()
}

pub fn binding_key(issuer: &str, subject: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update((issuer.len() as u64).to_be_bytes());
    hash.update(issuer);
    hash.update(subject);
    format!("{:x}", hash.finalize())
}

/// Trusted provisioning helper: emits only a non-secret local identity binding.
pub fn local_link(person_variable: &str, subject: &str) -> Result<String> {
    ensure!(
        person_variable.starts_with('$')
            && person_variable[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "Invalid identity variable"
    );
    let q = |s: &str| serde_json::to_string(s).unwrap();
    Ok(format!("(identity:{person_variable}) isa external_identity,has auth_binding_key {},has auth_issuer 'urn:jitpomi:local',has auth_subject {};",q(&binding_key("urn:jitpomi:local",subject)),q(subject)))
}

mod invitations;

mod account_lifecycle;
pub use account_lifecycle::AccountLifecycle;
