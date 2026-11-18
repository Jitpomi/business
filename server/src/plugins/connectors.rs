//! Provider adapters are installed capabilities, never workflow-engine dependencies.
use crate::automation::{
    connectors::{Connector, Invocation},
    model::{ActionSpec, Effect},
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::sync::Arc;
pub struct Builtin(pub &'static str);
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Required connection field missing"))
}
fn transport(kind: &str, ctx: &Invocation) -> Result<crate::plugins::builtin_transport::Providers> {
    let sandbox = ctx.config["sandbox"]
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("Choose sandbox or production explicitly"))?;
    let token = text(&ctx.secrets, "token")?.to_owned();
    Ok(crate::plugins::builtin_transport::Providers::connected(
        !sandbox,
        if kind == "stripe" {
            Some(token.clone())
        } else {
            None
        },
        if kind == "mercury" { Some(token) } else { None },
        ctx.config["connected_account"].as_str().map(str::to_owned),
        ctx.config["destination"].as_str().map(str::to_owned),
    )?
    .with_mercury_payment_options(
        ctx.config["card_enabled"] == true,
        ctx.config["ach_debit_enabled"] == true,
    ))
}
#[async_trait::async_trait]
impl Connector for Builtin {
    fn oauth(&self) -> Option<Arc<dyn crate::automation::oauth::Provider>> {
        (self.0 == "stripe").then(|| {
            Arc::new(super::stripe_oauth::Stripe) as Arc<dyn crate::automation::oauth::Provider>
        })
    }
    fn connection_purpose(&self) -> (&'static str, &'static str) {
        match self.0 {
            "stripe" | "mercury" => ("finance", "Payments and banking"),
            "resend" => ("communications", "Email and notifications"),
            _ => ("other", "Other systems"),
        }
    }
    fn connection_fields(&self) -> Value {
        let mut config = vec![
            json!({"key":"sandbox","label":"Environment","type":"environment","required":true}),
        ];
        match self.0 {
          "stripe" => config.push(json!({"key":"connected_account","label":"Connected account ID (optional)","type":"text"})),
          "mercury" => config.push(json!({"key":"destination","label":"Receiving account ID","type":"text","required":true})),
          "resend" => {config.push(json!({"key":"from","label":"Sender email","type":"text","required":true}));config.push(json!({"key":"allowed_test_recipients","label":"Allowed test emails (one per line)","type":"list"}));},
          _ => {}
        }
        let mut secrets =
            vec![json!({"key":"token","label":"API key","type":"password","required":true})];
        if self.0 != "resend" {
            secrets.push(json!({"key":"webhook_secret","label":"Webhook signing secret (optional)","type":"password"}));
        }
        json!({"config":config,"secrets":secrets})
    }
    fn verification_method(&self) -> &str {
        if self.0 == "resend" {
            "configuration"
        } else {
            "read_probe"
        }
    }
    fn name(&self) -> &str {
        self.0
    }
    fn actions(&self) -> Vec<ActionSpec> {
        if self.0 == "resend" {
            vec![ActionSpec {
                name: "send".into(),
                effect: Effect::AtMostOnce,
            }]
        } else {
            vec![ActionSpec {
                name: "observe_invoice".into(),
                effect: Effect::Read,
            }]
        }
    }
    fn validate_input(&self, config: &Value, action: &str, input: &Value) -> Result<()> {
        if self.0 == "resend" {
            ensure!(action == "send", "Unknown mail action");
            let to = text(input, "to")?;
            text(input, "subject")?;
            text(input, "text")?;
            if config["sandbox"] == true {
                ensure!(
                    config["allowed_test_recipients"]
                        .as_array()
                        .is_some_and(|a| a
                            .iter()
                            .any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(to)))),
                    "Sandbox recipient not approved"
                );
            }
        } else {
            ensure!(action == "observe_invoice", "Unknown financial action");
            crate::finance::opaque_id(text(input, "invoice_id")?)?;
        }
        Ok(())
    }
    fn validate(&self, c: &Value, s: &Value) -> Result<()> {
        reqwest::header::HeaderValue::from_str(text(s, "token")?)?;
        ensure!(
            s.as_object().is_some_and(|m| m
                .keys()
                .all(|k| ["token", "webhook_secret"].contains(&k.as_str()))),
            "Unknown credential field"
        );
        let allowed = match self.0 {
            "resend" => vec!["from", "allowed_test_recipients", "sandbox"],
            "stripe" => vec!["sandbox", "connected_account"],
            "mercury" => vec![
                "sandbox",
                "destination",
                "card_enabled",
                "ach_debit_enabled",
            ],
            _ => anyhow::bail!("Unknown built-in connector"),
        };
        for key in ["connected_account", "destination"] {
            ensure!(
                c.get(key).is_none_or(|v| v.is_null() || v.is_string()),
                "Account settings must be strings"
            );
        }
        for key in ["card_enabled", "ach_debit_enabled"] {
            ensure!(
                c.get(key).is_none_or(Value::is_boolean),
                "Payment options must be booleans"
            );
        }
        ensure!(
            c.as_object()
                .is_some_and(|m| m.keys().all(|k| allowed.contains(&k.as_str()))),
            "Unknown connection setting"
        );
        ensure!(c["sandbox"].is_boolean(), "Choose sandbox explicitly");
        if self.0 == "resend" {
            ensure!(
                text(s, "token")?.starts_with("re_") && text(s, "token")?.len() > 12,
                "Resend key format invalid"
            );
            ensure!(
                text(c, "from")?.len() <= 320 && !text(c, "from")?.chars().any(char::is_control),
                "Invalid sender"
            );
            ensure!(
                c.get("allowed_test_recipients")
                    .is_none_or(|a| a.as_array().is_some_and(|a| a.len() <= 100
                        && a.iter().all(|v| v
                            .as_str()
                            .is_some_and(|s| s.len() <= 254 && !s.chars().any(char::is_control))))),
                "Invalid sandbox recipients"
            );
        } else {
            transport(
                self.0,
                &Invocation {
                    tenant: uuid::Uuid::nil().to_string(),
                    run: String::new(),
                    step: String::new(),
                    key: String::new(),
                    config: c.clone(),
                    secrets: s.clone(),
                },
            )?;
        }
        Ok(())
    }
    async fn verify(&self, c: &Value, s: &Value) -> Result<()> {
        self.validate(c, s)?;
        // Sending-only keys cannot list domains. Validate their configuration
        // without demanding broader access or sending a verification email.
        if self.0 == "resend" {
            return Ok(());
        }
        crate::tls::initialize();
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(std::time::Duration::from_secs(25))
            .build()?;
        let url = match self.0 {
            "stripe" => "https://api.stripe.com/v1/account",
            "mercury" => {
                if c["sandbox"] == true {
                    "https://api-sandbox.mercury.com/api/v1/accounts"
                } else {
                    "https://api.mercury.com/api/v1/accounts"
                }
            }
            _ => "https://api.resend.com/domains",
        };
        let mut r = client.get(url).bearer_auth(text(s, "token")?);
        if self.0 == "stripe" {
            if let Some(account) = c["connected_account"].as_str() {
                r = r.header("Stripe-Account", account);
            }
        }
        let response = r
            .send()
            .await
            .map_err(|_| crate::automation::verification::Failure::Unavailable)?;
        crate::automation::verification::response(response.status())
    }
    fn financial(
        &self,
        ctx: &Invocation,
    ) -> Result<Option<Arc<dyn crate::finance::FinancialPlugin>>> {
        if self.0 == "resend" {
            return Ok(None);
        }
        Ok(Some(Arc::new(crate::plugins::BuiltinFinancial {
            transport: transport(self.0, ctx)?,
            provider: if self.0 == "stripe" {
                crate::plugins::builtin_transport::Provider::Stripe
            } else {
                crate::plugins::builtin_transport::Provider::Mercury
            },
            webhook_secret: ctx.secrets["webhook_secret"].as_str().map(str::to_owned),
        })))
    }
    fn notification(
        &self,
        ctx: &Invocation,
    ) -> Result<Option<Arc<dyn crate::finance::NotificationPlugin>>> {
        if self.0 != "resend" {
            return Ok(None);
        }
        Ok(Some(Arc::new(crate::plugins::Resend::new(
            text(&ctx.secrets, "token")?.into(),
            text(&ctx.config, "from")?.into(),
        )?)))
    }
    async fn execute(&self, ctx: &Invocation, action: &str, input: Value) -> Result<Value> {
        if self.0 == "resend" {
            ensure!(action == "send", "Unknown mail action");
            let to = text(&input, "to")?;
            if ctx.config["sandbox"] == true {
                ensure!(
                    ctx.config["allowed_test_recipients"]
                        .as_array()
                        .is_some_and(|a| a
                            .iter()
                            .any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(to)))),
                    "Sandbox recipient not approved"
                );
            }
            let id = self
                .notification(ctx)?
                .unwrap()
                .send(&crate::finance::Message {
                    to: to.into(),
                    subject: text(&input, "subject")?.into(),
                    text: text(&input, "text")?.into(),
                    key: ctx.key.clone(),
                })
                .await?;
            return Ok(json!({"delivery_id":id}));
        }
        ensure!(action == "observe_invoice", "Unknown financial action");
        let i = self
            .financial(ctx)?
            .unwrap()
            .observe_invoice(text(&input, "invoice_id")?)
            .await?;
        Ok(
            json!({"id":i.external_id,"customer":i.customer,"amount_minor":i.amount_minor,"currency":i.currency,"state":i.state,"payment_url":i.payment_url}),
        )
    }
}

/// Install the same trusted provider packages for the server and import tools.
pub fn register(catalog: &mut crate::automation::connectors::Catalog) -> Result<()> {
    for kind in ["stripe", "mercury", "resend"] {
        catalog.register(Arc::new(Builtin(kind)))?;
    }
    Ok(())
}
