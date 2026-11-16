//! One-use invitations and recovery on canonical identities. Tokens are hashed at rest.
use super::{billing_schema::quoted as q, engine::Engine};
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
impl Engine {
    async fn send_link(&self, email: &str, token: &str, kind: &str) -> Result<()> {
        ensure!(self.runtime.sandbox, "Sandbox only");
        let (subject, body) = if kind == "invite" {
            ("[TEST] Your JITPOMI account is ready",format!("You’re invited to JITPOMI. Set your password to see your company’s invoices and payments.\n\nhttps://jitpomi.com/account/#invite={token}\n\nThis private link works once and expires in 48 hours. Already have an account? Use your existing password.\n\nThis is a sandbox test. No real payments. Need help? Email dev@jitpomi.com."))
        } else {
            ("[TEST] Reset your JITPOMI password",format!("Choose a new password here:\n\nhttps://jitpomi.com/account/#reset={token}\n\nThis private link works once and expires in 30 minutes. If you didn’t ask for this, ignore this email. Your password has not changed.\n\nThis is a sandbox test. Need help? Email dev@jitpomi.com."))
        };
        self.runtime
            .send(crate::finance::Message {
                to: email.into(),
                subject: subject.into(),
                text: body,
                key: format!("account-{kind}-{}", digest(token)),
            })
            .await?;
        Ok(())
    }
    pub async fn invitation(&self, scope: &str, data: &Value) -> Result<Value> {
        let registry = crate::auth::Registry::configured(&self.state.database)?;
        let local = registry.local.clone();
        let action = field(data, "action")?;
        let secret = token();
        let key = digest(&secret);
        let invitation_id = uuid::Uuid::new_v4().to_string();
        let expires = now() + 172800;
        let (customer, buyer, address) = if action == "invite" {
            let name = field(data, "name")?.trim();
            ensure!(
                !name.is_empty() && name.len() <= 120 && !name.chars().any(char::is_control),
                "Enter a company name"
            );
            let address = email(data)?;
            let customer = uuid::Uuid::new_v4().to_string();
            let buyer = uuid::Uuid::new_v4().to_string();
            if let Some(store) = &local {
                store.issue(&key,&json!({"kind":"invite","email":address,"target":buyer,"version":key,"invitation":invitation_id,"expires":expires}))?;
            }
            self.write(format!(r#"{} {scope} not {{(merchant:$t,customer:$old) isa bill_account;$old has bill_email {};}};select $t;distinct;insert $buyer isa tenant,has tenant_id "{buyer}",has name {},has status "active",has revision "{key}";$c isa bill_customer,has bill_key "{customer}",has name {},has bill_email {},has bill_policy "customer",has bill_choice "manual",has bill_provider {},has bill_bindings "{{}}",has bill_method_plugin "",has bill_payment_method "",has bill_consent "",has bill_revision "{key}";(merchant:$t,buyer:$buyer,customer:$c) isa bill_account;$l isa account_invitation,has invitation_id "{invitation_id}",has invitation_email {},has invitation_target "{buyer}",has invitation_revision "{key}",has invitation_expires {expires},has invitation_used false;fetch {{"id":$c.bill_key}};"#,self.prefix(),q(&address),q(name),q(name),q(&address),q(self.runtime.default_financial.as_deref().unwrap_or("")),q(&address))).await?;
            (customer, buyer, address)
        } else {
            ensure!(
                matches!(action, "renew_invite" | "revoke_invite"),
                "Action unavailable"
            );
            let customer = super::billing_schema::id(field(data, "customer_id")?)?;
            let found=self.read(format!(r#"{} {scope} (merchant:$t,buyer:$b,customer:$c) isa bill_account;$c has bill_key "{customer}";fetch {{"buyer":$b.tenant_id,"email":$c.bill_email}};"#,self.prefix())).await?;
            ensure!(found.len() == 1, "Customer unavailable");
            let buyer = field(&found[0], "buyer")?.to_owned();
            let address = field(&found[0], "email")?.to_owned();
            if action == "revoke_invite" {
                self.write(format!(r#"{} {scope} $b isa tenant,has tenant_id "{buyer}";select $t,$b;distinct;update $b has revision "{key}";fetch {{"revoked":true}};"#,self.prefix())).await?;
                return Ok(json!({"revoked":true}));
            }
            if let Some(store) = &local {
                store.issue(&key,&json!({"kind":"invite","email":address,"target":buyer,"version":key,"invitation":invitation_id,"expires":expires}))?;
            }
            self.write(format!(r#"{} {scope} $b isa tenant,has tenant_id "{buyer}",has status "active";not {{(tenant:$b,member:$already) isa membership;}};select $t,$b;distinct;update $b has revision "{key}";insert $l isa account_invitation,has invitation_id "{invitation_id}",has invitation_email {},has invitation_target "{buyer}",has invitation_revision "{key}",has invitation_expires {expires},has invitation_used false;fetch {{"done":true}};"#,self.prefix(),q(&address))).await?;
            (customer, buyer, address)
        };
        let delivered = if local.is_some() {
            self.send_link(&address, &secret, "invite").await.is_ok()
        } else {
            false
        };
        let mut result = json!({"customer_id":customer,"tenant_id":buyer,"invitation_id":invitation_id,"expires_at":expires,"delivery":{"status":if delivered{"accepted"}else{"unconfirmed"}}});
        if local.is_some() {
            result["token"] = json!(secret);
        } else {
            result["authentication"] = json!({"flow":"provider","message":"Sign in with the configured identity provider before accepting this invitation"});
        }
        Ok(result)
    }
    pub async fn public_account_action(&self, kind: &str, data: &Value) -> Result<Value> {
        let registry = crate::auth::Registry::configured(&self.state.database)?;
        if registry.local.is_none() {
            return self.external_account_action(&registry, kind, data).await;
        }
        ensure!(
            self.runtime.sandbox,
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
            let _ = self.send_link(&address, &secret, "reset").await;
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
    async fn accept_identity_invitation(
        &self,
        invitation: &str,
        issuer: &str,
        subject: &str,
        address: &str,
    ) -> Result<Value> {
        let at = datetime();
        let binding = crate::auth::binding_key(issuer, subject);
        let invitation = super::billing_schema::id(invitation)?;
        let existing=self.read(format!("match $l isa external_identity,has auth_binding_key {},links(identity:$u);fetch {{'id':$u.identity_id}};",q(&binding))).await?;
        ensure!(existing.len() <= 1, "Ambiguous identity link");
        let contacts = if existing.is_empty() {
            self.read(format!("match $u isa person,has email {},has status 'active';not {{(identity:$u) isa external_identity;}};fetch {{'id':$u.identity_id}};",q(address))).await?
        } else {
            vec![]
        };
        ensure!(contacts.len() <= 1, "Ambiguous contact");
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
            (String::new(),format!("insert $u isa person,has identity_id {},has name {},has status 'active';$identity isa external_identity,links(identity:$u),has auth_binding_key {},has auth_issuer {},has auth_subject {};",q(&id),q(address),q(&binding),q(issuer),q(subject)))
        };
        let absent = if existing.is_empty() {
            String::new()
        } else {
            "not {(tenant:$b,member:$u) isa membership;};".into()
        };
        self.write(format!("{} $l isa account_invitation,has invitation_id {},has invitation_email {},has invitation_target $target,has invitation_revision $version,has invitation_expires $expiry,has invitation_used false;$expiry>{};$b isa tenant,has tenant_id == $target,has revision == $version,has status 'active';{person}{absent}{create}update $l has invitation_used true;insert (tenant:$b,member:$u) isa membership,has membership_role 'owner',has start_at {at};fetch {{'email':{}}};",self.prefix(),q(&invitation),q(address),now(),q(address))).await
    }
}
