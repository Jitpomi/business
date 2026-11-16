//! Authentication-owned one-use challenges and notification handoff. Domain
//! invitation creation, expiry and revocation are enforced by native policies.
use crate::{billing::billing_schema::quoted as q, services::BusinessParams};
use anyhow::{ensure, Result};
use dog_core::{DogError, DogService, TenantContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
impl crate::services::authentication::authentication_service::PluginAuthentication {
    pub async fn invitation(
        &self,
        ctx: &TenantContext,
        input: &Value,
        params: BusinessParams,
    ) -> Result<Value> {
        let proof = dog_auth::core::extract_bearer_token(&params.headers)
            .ok_or_else(|| DogError::not_authenticated("Sign in first").into_anyhow())?;
        let user = self
            .auth
            .create(ctx, json!({"strategy":"jwt","accessToken":proof}), params)
            .await?;
        let actor = user["user"]["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing identity"))?;
        let id = crate::billing::billing_schema::id(input["invitation_id"].as_str().unwrap_or(""))?;
        let now = chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6f")
            .to_string();
        let rows=crate::storage::read(&self.state,&format!("match $l isa account_invitation,has invitation_id {},has invitation_used false,has invitation_target $target,has invitation_revision $revision,has invitation_valid_until $until;$until>{now};$b isa tenant,has tenant_id == $target,has revision == $revision;let $allowed=scope_invitation({},{},{now},$l);$allowed==true;fetch {{'email':$l.invitation_email,'target':$target,'revision':$revision,'valid_until':$l.invitation_valid_until}};",q(&id),q(&ctx.tenant_id.0),q(actor))).await?;
        ensure!(
            rows.len() == 1,
            DogError::forbidden("Invitation unavailable or expired").into_anyhow()
        );
        let row = &rows[0];
        let expires = chrono::NaiveDateTime::parse_from_str(
            row["valid_until"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing invitation expiry"))?,
            "%Y-%m-%dT%H:%M:%S%.f",
        )?
        .and_utc()
        .timestamp();
        let runtime = self.financial.resolve(&ctx.tenant_id.0).await?;
        let mut token = id.clone();
        if let Some(local) = &self.plugins.local {
            token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let digest = format!("{:x}", Sha256::digest(token.as_bytes()));
            local.issue(&digest,&json!({"kind":"invite","email":row["email"],"target":row["target"],"version":row["revision"],"invitation":id,"expires":expires}))?;
        }
        let managed = self.plugins.local.is_none();
        let delivered=runtime.send(crate::finance::Message{to:row["email"].as_str().unwrap().into(),subject:format!("{}Your JITPOMI invitation",if runtime.sandbox{"[TEST] "}else{""}),text:format!("You’re invited to work with JITPOMI. {}\n\nhttps://jitpomi.com/account/#invite={token}\n\nThis invitation expires in 48 hours. Need a hand? Reply and we’ll help.",if managed{"Sign in with your own account to accept."}else{"Use your own existing password, or choose one for your new account."}),key:format!("invitation-{id}")}).await.is_ok();
        Ok(
            json!({"invitation_id":id,"token":token,"authentication":{"flow":if managed{"provider"}else{"local"}},"delivery":{"status":if delivered{"accepted"}else{"unconfirmed"}},"expires_at":expires}),
        )
    }
    pub async fn accept_invitation(&self, input: &Value) -> Result<Value> {
        let id = crate::billing::billing_schema::id(input["invitation_id"].as_str().unwrap_or(""))?;
        let proof = input["accessToken"]
            .as_str()
            .ok_or_else(|| DogError::bad_request("Missing provider proof").into_anyhow())?;
        let verified = self
            .plugins
            .verify(proof)
            .await
            .map_err(|_| DogError::not_authenticated("Invalid provider proof").into_anyhow())?;
        ensure!(
            verified.email_verified,
            DogError::forbidden("Verify your invitation email with your identity provider first")
                .into_anyhow()
        );
        let email = verified
            .email
            .as_deref()
            .ok_or_else(|| DogError::forbidden("Provider did not verify an email").into_anyhow())?;
        let now = chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6f")
            .to_string();
        let rows=crate::storage::read(&self.state,&format!("match $l isa account_invitation,has invitation_id {};$merchant isa tenant;let $b=invitation_target_for_email($merchant,$l,{},{});fetch {{'merchant':$merchant.tenant_id}};",q(&id),q(email),now)).await?;
        ensure!(
            rows.len() == 1,
            DogError::forbidden("Invitation unavailable").into_anyhow()
        );
        let merchant = rows[0]["merchant"].as_str().unwrap();
        super::account_lifecycle::accept_identity_invitation(
            &self.state,
            merchant,
            &id,
            &verified.issuer,
            &verified.subject,
            email,
        )
        .await
    }
}
