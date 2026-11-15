use crate::finance::{Message, NotificationPlugin};
use anyhow::{ensure, Result};
use async_trait::async_trait;
pub struct Resend {
    http: reqwest::Client,
    token: String,
    from: String,
}
impl Resend {
    pub fn new(token: String, from: String) -> Result<Self> {
        ensure!(
            !token.trim().is_empty() && !from.trim().is_empty(),
            "Mail credentials missing"
        );
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            http: reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(std::time::Duration::from_secs(25))
                .build()?,
            token,
            from,
        })
    }
}
#[async_trait]
impl NotificationPlugin for Resend {
    async fn send(&self, m: &Message) -> Result<String> {
        let response=self.http.post("https://api.resend.com/emails").bearer_auth(&self.token)
            .header("Idempotency-Key",&m.key).json(&serde_json::json!({"from":self.from,"to":[m.to],"subject":m.subject,"text":m.text})).send().await?;
        ensure!(
            response.status().is_success(),
            "Email send outcome requires reconciliation"
        );
        let value: serde_json::Value = response.json().await?;
        Ok(value["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Email delivery reference missing"))?
            .into())
    }
}
