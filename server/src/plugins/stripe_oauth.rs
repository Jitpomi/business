//! Stripe-specific OAuth protocol adapter.
use crate::automation::oauth::{Grant, Provider};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
fn bad(message: &str) -> anyhow::Error {
    dog_core::DogError::bad_request(message).into_anyhow()
}
/// Stripe Connect uses the platform key with a provider-authenticated account
/// binding, as recommended by Stripe. Deprecated OAuth bearer tokens are unused.
pub struct Stripe;
fn stripe_settings() -> Result<(String, String, String)> {
    let client = std::env::var("STRIPE_CONNECT_CLIENT_ID").map_err(|_| {
        bad("Stripe OAuth needs an app registration. API-key connections remain available.")
    })?;
    let secret = std::env::var("STRIPE_CONNECT_PLATFORM_KEY")
        .map_err(|_| bad("Stripe OAuth is not configured"))?;
    let redirect = std::env::var("CONNECTION_OAUTH_REDIRECT_URI")
        .map_err(|_| bad("OAuth callback is not configured"))?;
    let url = reqwest::Url::parse(&redirect)?;
    ensure!(
        client.starts_with("ca_")
            && (secret.starts_with("sk_test_") || secret.starts_with("sk_live_")),
        bad("Invalid Stripe OAuth configuration")
    );
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/account/connect-callback.html",
        bad("OAuth needs an exact HTTPS callback")
    );
    Ok((client, secret, redirect))
}
#[async_trait::async_trait]
impl Provider for Stripe {
    fn describe(&self) -> Value {
        json!({"configured":stripe_settings().is_ok(),"permissions":["Read and manage your Stripe account’s payments and invoices"],"pkce":false})
    }
    fn authorize(&self, state: &str, _verifier: &str) -> Result<String> {
        let (client, _, redirect) = stripe_settings()?;
        let mut url = reqwest::Url::parse("https://connect.stripe.com/oauth/authorize")?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", client.as_str()),
            ("response_type", "code"),
            ("scope", "read_write"),
            ("redirect_uri", redirect.as_str()),
            ("state", state),
        ]);
        Ok(url.into())
    }
    async fn exchange(&self, code: &str, _verifier: &str) -> Result<Grant> {
        let (_, secret, redirect) = stripe_settings()?;
        crate::tls::initialize();
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(std::time::Duration::from_secs(25))
            .build()?;
        let mut response = client
            .post("https://connect.stripe.com/oauth/token")
            .basic_auth(&secret, Some(""))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect.as_str()),
            ])
            .send()
            .await
            .map_err(|_| {
                bad("Provider outcome could not be confirmed. Start a new connection request.")
            })?;
        ensure!(
            response.status().is_success(),
            bad("Stripe did not approve this connection. Start a new request.")
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 65536,
                "OAuth response exceeded limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| bad("Invalid provider response"))?;
        let public_key = std::env::var("STRIPE_CONNECT_PLATFORM_PUBLISHABLE_KEY").ok();
        stripe_grant(&value, &secret, public_key.as_deref())
    }
}
fn stripe_grant(value: &Value, secret: &str, public_key: Option<&str>) -> Result<Grant> {
    let account = value["stripe_user_id"]
        .as_str()
        .filter(|s| {
            s.starts_with("acct_")
                && s.len() < 128
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .ok_or_else(|| bad("Stripe did not identify the account"))?;
    let live = value["livemode"]
        .as_bool()
        .ok_or_else(|| bad("Stripe did not identify the environment"))?;
    ensure!(
        live == secret.starts_with("sk_live_") && value["scope"] == "read_write",
        bad("Unexpected Stripe environment or permission")
    );
    let mut config = json!({"sandbox":!live,"connected_account":account});
    // Stripe recommends the platform publishable key + connected-account
    // binding. Deprecated OAuth publishable/access tokens are not consumed.
    if let Some(key) = public_key {
        crate::plugins::builtin_transport::validate_publishable_key(key, live)?;
        config["publishable_key"] = json!(key);
    }
    Ok(Grant {
        config,
        secrets: json!({"token":secret}),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stripe_account_is_provider_bound() {
        let v = json!({"stripe_user_id":"acct_test","livemode":false,"scope":"read_write","access_token":"unused"});
        let g = stripe_grant(&v, "sk_test_private", None).unwrap();
        assert_eq!(
            g.config,
            json!({"sandbox":true,"connected_account":"acct_test"})
        );
        assert_eq!(g.secrets, json!({"token":"sk_test_private"}));
        for v in [
            json!({}),
            json!({"stripe_user_id":"acct_bad\r\n","livemode":false,"scope":"read_write"}),
            json!({"stripe_user_id":"acct_ok","livemode":true,"scope":"read_write"}),
            json!({"stripe_user_id":"acct_ok","livemode":false,"scope":"read_only"}),
        ] {
            assert!(stripe_grant(&v, "sk_test_private", None).is_err());
        }
        let granted = stripe_grant(&v, "sk_test_private", Some("pk_test_platform123456")).unwrap();
        assert_eq!(granted.config["publishable_key"], "pk_test_platform123456");
        assert!(stripe_grant(&v, "sk_test_private", Some("pk_live_platform123456")).is_err());
    }
}
