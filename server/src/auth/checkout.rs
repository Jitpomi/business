//! Hosted-provider proof enrollment; credentials never enter the graph.
use crate::{services::authentication::authentication_service::PluginAuthentication, storage};
use anyhow::{ensure, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}
impl PluginAuthentication {
    pub(crate) async fn checkout_enrolment(&self, input: &Value) -> Result<Value> {
        let merchant =
            crate::billing::billing_schema::id(input["merchant_id"].as_str().unwrap_or(""))?;
        let verified = self
            .plugins
            .verify(input["accessToken"].as_str().unwrap_or(""))
            .await
            .map_err(|_| {
                dog_core::DogError::not_authenticated("Invalid provider proof").into_anyhow()
            })?;
        ensure!(
            verified.email_verified,
            dog_core::DogError::forbidden(
                "Verify your email with your sign-in provider before checking out"
            )
            .into_anyhow()
        );
        let email = verified
            .email
            .as_deref()
            .filter(|s| !s.is_empty() && s.len() <= 254)
            .ok_or_else(|| {
                dog_core::DogError::forbidden("A verified contact email is required").into_anyhow()
            })?;
        ensure!(
            self.financial.configuration(&merchant).await?.sandbox,
            dog_core::DogError::forbidden("Real checkout payments are disabled").into_anyhow()
        );
        let binding = crate::auth::binding_key(&verified.issuer, &verified.subject);
        let linked=storage::read(&self.state,&format!("match $link isa external_identity,has auth_binding_key {},links(identity:$p);fetch {{'person':$p.identity_id}};",q(&binding))).await?;
        ensure!(linked.len() <= 1, "Ambiguous external identity");
        let person = linked
            .first()
            .and_then(|r| r["person"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let claim = format!(
            "{:x}",
            Sha256::digest(format!("{merchant}:{person}").as_bytes())
        );
        let lookup = || {
            format!("match $t isa tenant,has tenant_id {}; $s isa commerce_shopper,has cx_id {},links(merchant:$t,person:$person,customer:$c);$person isa person,has identity_id {}; (buyer:$b,customer:$c) isa bill_account;let $allowed=bill_buyer($person,$b,$c,{});$allowed==true;fetch {{'tenant_id':$b.tenant_id,'customer_id':$c.bill_key}};",q(&merchant),q(&claim),q(&person),crate::clock::native_utc())
        };
        if let Some(existing) = storage::read(&self.state, &lookup()).await?.first() {
            return Ok(existing.clone());
        }
        let buyer = uuid::Uuid::new_v4().to_string();
        let customer = uuid::Uuid::new_v4().to_string();
        let now = crate::clock::native_utc();
        let identity = if linked.is_empty() {
            format!("insert $person isa person,has identity_id {},has name 'Customer',has status 'active';(identity:$person) isa external_identity,has auth_binding_key {},has auth_issuer {},has auth_subject {};",q(&person),q(&binding),q(&verified.issuer),q(&verified.subject))
        } else {
            format!("match $person isa person,has identity_id {};", q(&person))
        };
        let query=format!("match $t isa tenant,has tenant_id {};not {{$s isa commerce_shopper,has cx_id {};}};{identity}match let $allowed=cx_self_enrolment($t,$person,{now});$allowed==true;select $t,$person;distinct;insert $b isa tenant,has tenant_id {},has name 'Your purchases',has status 'active',has revision 'initial';(tenant:$b,member:$person) isa membership,has membership_role 'owner',has start_at {now};$c isa bill_customer,has bill_key {},has bill_customer_claim {},has name 'Customer',has bill_email {},has bill_policy 'customer',has bill_choice 'manual',has bill_provider '',has bill_bindings '{{}}',has bill_method_plugin '',has bill_payment_method '',has bill_consent '',has bill_revision 'initial';(merchant:$t,buyer:$b,customer:$c) isa bill_account;$s isa commerce_shopper,has cx_id {},links(merchant:$t,person:$person,customer:$c);fetch {{'tenant_id':$b.tenant_id,'customer_id':$c.bill_key}};",q(&merchant),q(&claim),q(&buyer),q(&customer),q(&format!("{merchant}:shopper:{person}")),q(email),q(&claim));
        match storage::write(&self.state, &query).await {
            Ok(result) => Ok(result),
            Err(error) => {
                // Resolve an uncertain enrollment by its immutable identity/merchant claim.
                // Do not retry the write or grant new membership after revocation.
                if let Some(existing) = storage::read(&self.state, &lookup()).await?.first() {
                    Ok(existing.clone())
                } else {
                    Err(error)
                }
            }
        }
    }
}
