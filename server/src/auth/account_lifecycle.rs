//! One-use invitations and recovery on canonical identities. Tokens are hashed at rest.
use crate::typedb::TypeDBState;
use std::sync::Arc;
fn q(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn datetime() -> String {
    chrono::Utc::now()
        .naive_utc()
        .format("%Y-%m-%dT%H:%M:%S%.6f")
        .to_string()
}
fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().ok_or_else(|| anyhow::anyhow!("Missing {k}"))
}
fn email(v: &Value) -> Result<String> {
    let s = field(v, "email")?.trim().to_ascii_lowercase();
    ensure!(
        s.len() <= 254
            && s.split_once('@')
                .is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.contains('@'))
            && !s.chars().any(|c| c.is_control() || c.is_whitespace()),
        "Enter a valid email address"
    );
    Ok(s)
}
async fn hash(password: String) -> Result<String> {
    ensure!(
        (12..=72).contains(&password.len()) && !password.trim().is_empty(),
        "Use 12–72 characters for your password"
    );
    Ok(tokio::task::spawn_blocking(move || bcrypt::hash(password, bcrypt::DEFAULT_COST)).await??)
}
pub struct AccountLifecycle {
    state: Arc<TypeDBState>,
    tenant: String,
    notification: Arc<crate::finance::TenantRuntime>,
}
impl AccountLifecycle {
    pub fn new(
        state: Arc<TypeDBState>,
        tenant: &str,
        notification: Arc<crate::finance::TenantRuntime>,
    ) -> Result<Self> {
        let tenant = uuid::Uuid::parse_str(tenant)?.to_string();
        ensure!(
            notification.tenant == tenant,
            "Account notification scope mismatch"
        );
        Ok(Self {
            state,
            tenant,
            notification,
        })
    }
    async fn send_recovery_link(&self, email: &str, token: &str) -> Result<()> {
        ensure!(self.notification.sandbox, "Sandbox only");
        let subject = "[TEST] Reset your JITPOMI password";
        let body=format!("Choose a new password here:\n\nhttps://jitpomi.com/account/#reset={token}\n\nThis private link works once and expires in 30 minutes. If you didn’t ask for this, ignore this email. Your password has not changed.\n\nThis is a sandbox test. Need help? Email dev@jitpomi.com.");
        self.notification
            .send(crate::finance::Message {
                to: email.into(),
                subject: subject.into(),
                text: body,
                key: format!("account-reset-{}", digest(token)),
            })
            .await?;
        Ok(())
    }
    pub async fn public_account_action(&self, kind: &str, data: &Value) -> Result<Value> {
        let registry = crate::auth::Registry::configured(&self.state.database)?;
        if registry.local.is_none() {
            return self.external_account_action(&registry, kind, data).await;
        }
        ensure!(
            self.notification.sandbox,
            "Local account onboarding is sandbox-only"
        );
        let store = registry.local()?;
        if kind == "reset" && data["action"] == "request" {
            let address = email(data)?;
            let answer = json!({"requested":true,"message":"If you have an account, we’ll email you a link to reset your password. Check your inbox and spam folder."});
            let Some(account) = store.lookup(&address, true)? else {
                return Ok(answer);
            };
            if store.recent("reset", &address, now() + 1500)? {
                return Ok(answer);
            }
            let secret = token();
            let expires = now() + 1800;
            store.issue(&digest(&secret),&json!({"kind":"reset","email":address,"target":account["subject"],"version":account["version"],"expires":expires}))?;
            let _ = self.send_recovery_link(&address, &secret).await;
            return Ok(answer);
        }
        let secret = field(data, "token")?;
        ensure!(
            secret.len() == 64 && secret.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid account link"
        );
        let key = digest(secret);
        let link = store
            .challenge(&key)?
            .ok_or_else(|| anyhow::anyhow!("Invalid or used account link"))?;
        ensure!(
            link["kind"] == kind && link["expires"].as_i64().is_some_and(|v| v > now()),
            "Expired account link"
        );
        let address = field(&link, "email")?;
        let target = field(&link, "target")?;
        let password = field(data, "password")?.to_owned();
        if kind == "reset" {
            ensure!(data["confirm_password"] == password, "Passwords must match");
            let hashed = hash(password).await?;
            store.reset(
                &key,
                target,
                field(&link, "version")?,
                &hashed,
                &uuid::Uuid::new_v4().to_string(),
            )?;
            return Ok(json!({"reset":true}));
        }
        ensure!(kind == "invite", "Action unavailable");
        let at = datetime();
        let valid=crate::storage::read(&self.state,&format!("match $t isa tenant,has tenant_id {}; $l isa account_invitation,has invitation_id {}; let $b=invitation_target_for_email($t,$l,{},{});fetch {{'id':$b.tenant_id}};",q(&self.tenant),q(field(&link,"invitation")?),q(address),at)).await?;
        ensure!(
            valid.len() == 1,
            "Invitation expired, revoked or already used"
        );
        let account = if let Some(account) = store.lookup(address, true)? {
            let hashed = field(&account, "password")?.to_owned();
            ensure!(
                tokio::task::spawn_blocking(move || bcrypt::verify(password, &hashed)).await??,
                "Use your existing account password"
            );
            account
        } else {
            let hashed = hash(password).await?;
            let subject = uuid::Uuid::new_v4().to_string();
            let version = uuid::Uuid::new_v4().to_string();
            store.import(&subject, address, &hashed, &version)?;
            store.lookup(&subject, false)?.unwrap()
        };
        let subject = field(&account, "subject")?;
        let result = self
            .accept_identity_invitation(
                field(&link, "invitation")?,
                "urn:jitpomi:local",
                subject,
                address,
            )
            .await?;
        // Graph acceptance is authoritative. A failed store acknowledgement cannot
        // repeat membership creation because the graph invitation is one-use.
        let _ = store.consume(&key)?;
        Ok(result)
    }
    async fn external_account_action(
        &self,
        registry: &crate::auth::Registry,
        kind: &str,
        data: &Value,
    ) -> Result<Value> {
        if kind == "reset" {
            let id = field(data, "plugin")?;
            let plugin = registry.plugin(id)?;
            return Ok(json!({"flow":"provider","account_url":plugin.account_url()}));
        }
        ensure!(kind == "invite", "Action unavailable");
        let verified = registry.verify(field(data, "accessToken")?).await?;
        ensure!(
            verified.email_verified,
            "The provider must verify the invitation email"
        );
        self.accept_identity_invitation(
            field(data, "invitation_id")?,
            &verified.issuer,
            &verified.subject,
            verified
                .email
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Provider did not supply a verified email"))?,
        )
        .await
    }
    pub(crate) async fn accept_identity_invitation(
        &self,
        invitation: &str,
        issuer: &str,
        subject: &str,
        address: &str,
    ) -> Result<Value> {
        accept_identity_invitation(
            &self.state,
            &self.tenant,
            invitation,
            issuer,
            subject,
            address,
        )
        .await
    }
}
pub(crate) async fn accept_identity_invitation(
    state: &Arc<TypeDBState>,
    tenant: &str,
    invitation: &str,
    issuer: &str,
    subject: &str,
    address: &str,
) -> Result<Value> {
    let at = datetime();
    let binding = crate::auth::binding_key(issuer, subject);
    let invitation = uuid::Uuid::parse_str(invitation)?.to_string();
    let existing=crate::storage::read(state,&format!("match $l isa external_identity,has auth_binding_key {},links(identity:$u);fetch {{'id':$u.identity_id}};",q(&binding))).await?;
    ensure!(existing.len() <= 1, "Ambiguous identity link");
    let contacts = if existing.is_empty() {
        crate::storage::read(state,&format!("match $u isa person,has email {},has status 'active';not {{(identity:$u) isa external_identity;}};fetch {{'id':$u.identity_id}};",q(address))).await?
    } else {
        vec![]
    };
    ensure!(contacts.len() <= 1, "Ambiguous contact");
    if existing.is_empty() && contacts.is_empty() {
        let linked=crate::storage::read(state,&format!("match $u isa person,has email {}; (identity:$u) isa external_identity;fetch {{'id':$u.identity_id}};",q(address))).await?;
        ensure!(linked.is_empty(),dog_core::DogError::conflict("Sign in with your existing account and link this provider before accepting the invitation").into_anyhow());
    }

    let (person, create) = if let Some(user) = existing.first() {
        (
            format!(
                "$u isa person,has identity_id {},has status 'active';",
                q(field(user, "id")?)
            ),
            String::new(),
        )
    } else if let Some(contact) = contacts.first() {
        (format!("$u isa person,has identity_id {},has status 'active';not {{(identity:$u) isa external_identity;}};",q(field(contact,"id")?)),format!("insert $identity isa external_identity,links(identity:$u),has auth_binding_key {},has auth_issuer {},has auth_subject {};",q(&binding),q(issuer),q(subject)))
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        (String::new(),format!("insert $u isa person,has identity_id {},has name {},has email {},has status 'active';$identity isa external_identity,links(identity:$u),has auth_binding_key {},has auth_issuer {},has auth_subject {};",q(&id),q(address),q(address),q(&binding),q(issuer),q(subject)))
    };
    let absent = if existing.is_empty() {
        String::new()
    } else {
        "not {(tenant:$b,member:$u) isa membership;};".into()
    };
    let prefix = format!(
        "match $t isa tenant,has tenant_id {},has status 'active',has revision $revision;",
        q(tenant)
    );
    crate::storage::write(state,&format!("{prefix} $l isa account_invitation,has invitation_id {};let $b=invitation_target_for_email($t,$l,{},{at});{person}{absent}{create}update $l has invitation_used true;insert (tenant:$b,member:$u) isa membership,has membership_role 'owner',has start_at {at};fetch {{'email':{}}};",q(&invitation),q(address),q(address))).await
}
