//! Configured JWT access-token verifier. Browser login/account UI stays with the IdP.
use anyhow::{ensure, Result};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub id: String,
    pub issuer: String,
    pub audience: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub account_url: Option<String>,
    #[serde(default = "token_types")]
    pub token_types: Vec<String>,
    #[serde(default = "email_claim")]
    pub email_claim: String,
    #[serde(default = "email_verified_claim")]
    pub email_verified_claim: String,
    #[serde(default = "amr_claim")]
    pub amr_claim: String,
    #[serde(default = "auth_time_claim")]
    pub auth_time_claim: String,
    #[serde(default = "acr_claim")]
    pub acr_claim: String,
    #[serde(default = "max_lifetime")]
    pub max_token_lifetime_seconds: i64,
}
fn amr_claim() -> String {
    "amr".into()
}
fn auth_time_claim() -> String {
    "auth_time".into()
}
fn acr_claim() -> String {
    "acr".into()
}
fn max_lifetime() -> i64 {
    900
}
fn email_claim() -> String {
    "email".into()
}
fn email_verified_claim() -> String {
    "email_verified".into()
}
fn token_types() -> Vec<String> {
    vec!["at+jwt".into()]
}
pub struct Oidc {
    pub config: Config,
    client: reqwest::Client,
    keys: tokio::sync::Mutex<Option<(std::time::Instant, Arc<JwkSet>)>>,
}
impl Oidc {
    pub fn new(config: Config) -> Result<Self> {
        crate::tls::initialize();
        for value in [&config.issuer, &config.jwks_uri]
            .into_iter()
            .chain(config.account_url.as_ref())
        {
            let u = reqwest::Url::parse(value)?;
            ensure!(
                u.scheme() == "https"
                    && u.host_str().is_some()
                    && u.username().is_empty()
                    && u.password().is_none()
                    && u.fragment().is_none(),
                "Trusted HTTPS provider URLs required"
            );
        }
        ensure!(
            (60..=3600).contains(&config.max_token_lifetime_seconds),
            "API token lifetime must be 60–3600 seconds"
        );
        ensure!(
            !config.audience.is_empty()
                && !config.token_types.is_empty()
                && config
                    .token_types
                    .iter()
                    .all(|v| matches!(v.as_str(), "at+jwt" | "JWT")),
            "Explicit API audience and token types required"
        );
        Ok(Self {
            config,
            client: reqwest::Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            keys: tokio::sync::Mutex::new(None),
        })
    }
    /// Trusted hosts may supply a TLS client (for private enterprise CAs).
    pub fn with_client(config: Config, client: reqwest::Client) -> Result<Self> {
        let mut plugin = Self::new(config)?;
        plugin.client = client;
        Ok(plugin)
    }
    async fn keys(&self) -> Result<Arc<JwkSet>> {
        let mut cache = self.keys.lock().await;
        if let Some((at, keys)) = cache.as_ref() {
            if at.elapsed().as_secs() < 60 {
                return Ok(keys.clone());
            }
        }
        let response = self
            .client
            .get(&self.config.jwks_uri)
            .send()
            .await?
            .error_for_status()?;
        ensure!(
            response.content_length().unwrap_or(0) <= 262144,
            "Provider key set exceeds limit"
        );
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 262144,
                "Provider key set exceeds limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let keys: JwkSet = serde_json::from_slice(&bytes)?;
        ensure!(keys.keys.len() <= 32, "Too many provider keys");
        let keys = Arc::new(keys);
        *cache = Some((std::time::Instant::now(), keys.clone()));
        Ok(keys)
    }
    pub async fn verify(&self, token: &str) -> Result<super::super::Verified> {
        ensure!(token.len() <= 32768, "Token exceeds limit");
        let header = decode_header(token)?;
        ensure!(
            matches!(
                header.alg,
                Algorithm::RS256 | Algorithm::ES256 | Algorithm::EdDSA
            ),
            "Unsupported signing algorithm"
        );
        ensure!(
            header
                .typ
                .as_ref()
                .is_some_and(|t| self.config.token_types.contains(t)),
            "Wrong token type"
        );
        let kid = header
            .kid
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Missing signing key ID"))?;
        let keys = self.keys().await?;
        let jwk = keys
            .find(kid)
            .ok_or_else(|| anyhow::anyhow!("Unknown signing key"))?;
        ensure!(
            jwk.common
                .public_key_use
                .as_ref()
                .is_none_or(|v| matches!(v, jsonwebtoken::jwk::PublicKeyUse::Signature)),
            "Key is not for signature verification"
        );
        ensure!(
            jwk.common
                .key_operations
                .as_ref()
                .is_none_or(|v| v.contains(&jsonwebtoken::jwk::KeyOperations::Verify)),
            "Key does not permit verification"
        );
        ensure!(
            jwk.common.key_algorithm.as_ref().is_none_or(
                |a| serde_json::to_value(a).ok() == serde_json::to_value(header.alg).ok()
            ),
            "Signing algorithm differs from key declaration"
        );
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "sub", "aud", "iat"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let mut claims = decode::<Value>(token, &DecodingKey::from_jwk(jwk)?, &validation)?.claims;
        let now = crate::clock::unix_seconds();
        ensure!(
            claims["iat"].as_i64().is_some_and(|v| v <= now + 30),
            "Invalid issue time"
        );
        let issued = claims["iat"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Invalid issue time"))?;
        let expires = claims["exp"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Invalid expiry"))?;
        ensure!(
            expires > issued
                && expires.saturating_sub(issued) <= self.config.max_token_lifetime_seconds,
            "API token lifetime exceeds policy"
        );
        let amr = claims[&self.config.amr_claim].clone();
        let auth_time = claims[&self.config.auth_time_claim].clone();
        let acr = claims[&self.config.acr_claim].clone();
        if amr.is_null() {
            claims.as_object_mut().unwrap().remove("amr");
        } else {
            claims["amr"] = amr;
        }
        if auth_time.is_null() {
            claims.as_object_mut().unwrap().remove("auth_time");
        } else {
            claims["auth_time"] = auth_time;
        }
        claims["acr"] = acr;
        let email = claims[&self.config.email_claim]
            .as_str()
            .map(|s| s.trim().to_ascii_lowercase());
        let verified = claims[&self.config.email_verified_claim] == true;
        claims["email"] = serde_json::to_value(email)?;
        claims["email_verified"] = Value::Bool(verified);
        super::super::Verified::from_claims(&self.config.id, &claims)
    }
}
