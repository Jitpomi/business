//! Trusted connector contract. Connector code never decides tenant ownership or retries.
use super::model::{ActionSpec, Effect};
use anyhow::{ensure, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone)]
pub struct Invocation {
    pub tenant: String,
    pub run: String,
    pub step: String,
    pub key: String,
    pub config: Value,
    pub secrets: Value,
}
#[async_trait]
pub trait Connector: Send + Sync {
    /// Provider-owned onboarding; core execution never knows provider protocols.
    fn oauth(&self) -> Option<Arc<dyn super::oauth::Provider>> {
        None
    }
    /// Engine capabilities can stay in the action catalog without appearing as external systems.
    fn internal(&self) -> bool {
        false
    }
    fn connection_purpose(&self) -> (&'static str, &'static str) {
        ("other", "Other systems")
    }
    /// Optional UI field descriptions. Unknown installed plugins use JSON objects.
    fn connection_fields(&self) -> Value {
        Value::Null
    }
    fn name(&self) -> &str;
    fn validate_input(&self, _config: &Value, _action: &str, _input: &Value) -> Result<()> {
        Ok(())
    }
    fn timeout_seconds(&self) -> u64 {
        30
    }
    fn financial(
        &self,
        _: &Invocation,
    ) -> Result<Option<Arc<dyn crate::finance::FinancialPlugin>>> {
        Ok(None)
    }
    fn notification(
        &self,
        _: &Invocation,
    ) -> Result<Option<Arc<dyn crate::finance::NotificationPlugin>>> {
        Ok(None)
    }
    fn actions(&self) -> Vec<ActionSpec>;
    fn verification_method(&self) -> &str {
        "read_probe"
    }
    fn validate(&self, config: &Value, secrets: &Value) -> Result<()>;
    async fn verify(&self, config: &Value, secrets: &Value) -> Result<()>;
    async fn execute(&self, ctx: &Invocation, action: &str, input: Value) -> Result<Value>;
}
#[derive(Clone, Default)]
pub struct Catalog {
    entries: BTreeMap<String, Arc<dyn Connector>>,
}
impl Catalog {
    pub fn register(&mut self, c: Arc<dyn Connector>) -> Result<()> {
        super::model::identifier(c.name())?;
        ensure!(
            (1..=300).contains(&c.timeout_seconds()),
            "Connector timeout must be 1..300 seconds"
        );
        ensure!(c.actions().len() <= 100, "Connector action limit exceeded");
        ensure!(
            !self.entries.contains_key(c.name()),
            "Duplicate connector kind"
        );
        let mut names = std::collections::HashSet::new();
        for a in c.actions() {
            super::model::identifier(&a.name)?;
            ensure!(names.insert(a.name), "Duplicate connector action");
        }
        self.entries.insert(c.name().into(), c);
        Ok(())
    }
    pub fn get(&self, name: &str) -> Result<Arc<dyn Connector>> {
        self.entries
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Connector is not installed"))
    }
    pub fn describe(&self) -> Value {
        json!(self
            .entries
            .values()
            .map(|c| json!({"id":c.name(),"actions":c.actions(),"verification":c.verification_method(),"oauth":c.oauth().map(|p| p.describe()),"fields":c.connection_fields(),"internal":c.internal(),"purpose":{"id":c.connection_purpose().0,"name":c.connection_purpose().1}}))
            .collect::<Vec<_>>())
    }
}
pub fn redact(value: Value, secrets: &Value) -> Value {
    fn collect(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) if !s.is_empty() => out.push(s.clone()),
            Value::Array(a) => {
                for v in a {
                    collect(v, out)
                }
            }
            Value::Object(o) => {
                for v in o.values() {
                    collect(v, out)
                }
            }
            _ => {}
        }
    }
    fn scrub(v: Value, keys: &[String]) -> Value {
        match v {
            Value::String(mut s) => {
                for k in keys {
                    s = s.replace(k, "[redacted]");
                }
                Value::String(s)
            }
            Value::Array(a) => Value::Array(a.into_iter().map(|v| scrub(v, keys)).collect()),
            Value::Object(o) => Value::Object(
                o.into_iter()
                    .map(|(k, v)| {
                        (
                            match scrub(Value::String(k), keys) {
                                Value::String(k) => k,
                                _ => unreachable!(),
                            },
                            scrub(v, keys),
                        )
                    })
                    .collect(),
            ),
            v => v,
        }
    }
    let mut keys = vec![];
    collect(secrets, &mut keys);
    keys.sort_by_key(|s| std::cmp::Reverse(s.len()));
    scrub(value, &keys)
}
/// Generic HTTPS is intentionally not an automatic retryable write adapter.
pub struct Http;
impl Http {
    fn origin(config: &Value) -> Result<reqwest::Url> {
        let url = reqwest::Url::parse(
            config["origin"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("HTTPS origin required"))?,
        )?;
        ensure!(
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.host_str().is_some()
                && url.port_or_known_default() == Some(443)
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "Use an HTTPS origin without a path or embedded credentials"
        );
        Ok(url)
    }
    pub fn public(ip: std::net::IpAddr) -> bool {
        match ip {
            std::net::IpAddr::V4(v) => {
                let a = v.octets();
                !v.is_private()
                    && !v.is_loopback()
                    && !v.is_link_local()
                    && !v.is_unspecified()
                    && !v.is_multicast()
                    && !v.is_broadcast()
                    && a[0] != 0
                    && a[0] < 224
                    && !(a[0] == 100 && (64..128).contains(&a[1]))
                    && !(a[0] == 198 && (a[1] == 18 || a[1] == 19))
                    && !(a[0] == 192
                        && ((a[1] == 0 && (a[2] == 0 || a[2] == 2)) || (a[1] == 88 && a[2] == 99)))
                    && !(a[0] == 169 && a[1] == 254)
                    && !(a[0] == 198 && a[1] == 51 && a[2] == 100)
                    && !(a[0] == 203 && a[1] == 0 && a[2] == 113)
            }
            std::net::IpAddr::V6(v) => {
                if let Some(v) = v.to_ipv4_mapped() {
                    return Self::public(v.into());
                }
                let s = v.segments();
                (s[0] & 0xe000) == 0x2000
                    && s[0] != 0x2002
                    && s[0] != 0x3fff
                    && !(s[0] == 0x2001 && (s[1] == 0xdb8 || s[1] < 0x200))
            }
        }
    }
    async fn client(url: &reqwest::Url) -> Result<reqwest::Client> {
        crate::tls::initialize();
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("Host required"))?;
        let addresses: Vec<_> = tokio::net::lookup_host((host, 443)).await?.collect();
        ensure!(
            !addresses.is_empty() && addresses.iter().all(|a| Self::public(a.ip())),
            "Only public HTTPS destinations are allowed"
        );
        Ok(reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(std::time::Duration::from_secs(25))
            .resolve_to_addrs(host, &addresses)
            .build()?)
    }
    async fn request(&self, ctx: &Invocation, action: &str, input: &Value) -> Result<Value> {
        let base = Self::origin(&ctx.config)?;
        let path = input["path"].as_str().unwrap_or("/");
        ensure!(
            path.starts_with('/') && !path.starts_with("//") && !path.contains(['\r', '\n', '#']),
            "Use a relative API path"
        );
        let url = base.join(path)?;
        ensure!(
            url.origin() == base.origin(),
            "Cross-origin request rejected"
        );
        let method = if action == "get" {
            reqwest::Method::GET
        } else {
            match input["method"].as_str().unwrap_or("POST") {
                "POST" => reqwest::Method::POST,
                "PUT" => reqwest::Method::PUT,
                "PATCH" => reqwest::Method::PATCH,
                "DELETE" => reqwest::Method::DELETE,
                _ => anyhow::bail!("Unsupported write method"),
            }
        };
        let client = Self::client(&url).await?;
        let mut request = client.request(method, url);
        if let Some(token) = ctx.secrets["bearer"].as_str() {
            request = request.bearer_auth(token);
        }
        if let Some(value) = ctx.secrets["api_key"].as_str() {
            let name = ctx.config["api_key_header"].as_str().unwrap_or("x-api-key");
            request = request.header(name, value);
        }
        if action == "send" {
            request = request.header("Idempotency-Key", &ctx.key);
            if let Some(body) = input.get("body") {
                request = request.json(body);
            }
        }
        let mut response = request.send().await?;
        let status = response.status();
        ensure!(status.is_success(), "External system rejected the request");
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 262144,
                "Connector response is too large"
            );
            bytes.extend(chunk);
        }
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
        Ok(json!({"status":status.as_u16(),"body":value}))
    }
}
#[async_trait]
impl Connector for Http {
    fn connection_fields(&self) -> Value {
        json!({
          "config":[{"key":"origin","label":"API origin (HTTPS)","type":"text","required":true},{"key":"verify_path","label":"Read-only verification path (optional)","type":"text"},{"key":"api_key_header","label":"API-key header name (optional)","type":"text"}],
          "secrets":[{"key":"bearer","label":"Bearer token (optional)","type":"password"},{"key":"api_key","label":"API key (optional)","type":"password"}]
        })
    }

    fn name(&self) -> &str {
        "http"
    }
    fn actions(&self) -> Vec<ActionSpec> {
        vec![
            ActionSpec {
                name: "get".into(),
                effect: Effect::Read,
            },
            ActionSpec {
                name: "send".into(),
                effect: Effect::AtMostOnce,
            },
        ]
    }
    fn validate_input(&self, _config: &Value, action: &str, input: &Value) -> Result<()> {
        ensure!(
            input.as_object().is_some_and(|o| o
                .keys()
                .all(|k| ["path", "method", "body"].contains(&k.as_str()))),
            "Unknown HTTP action field"
        );
        let path = input["path"].as_str().unwrap_or("/");
        ensure!(
            path.starts_with('/') && !path.starts_with("//") && !path.contains(['\r', '\n', '#']),
            "Use a relative API path"
        );
        ensure!(action == "get" || action == "send", "Unknown HTTP action");
        if action == "send" {
            ensure!(
                ["POST", "PUT", "PATCH", "DELETE"]
                    .contains(&input["method"].as_str().unwrap_or("POST")),
                "Unsupported write method"
            );
        }
        Ok(())
    }
    fn validate(&self, c: &Value, s: &Value) -> Result<()> {
        Self::origin(c)?;
        if let Some(o) = s.as_object() {
            for v in o.values() {
                let s = v
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 16384)
                    .ok_or_else(|| anyhow::anyhow!("Credential must be a nonempty string"))?;
                reqwest::header::HeaderValue::from_str(s)?;
            }
        }
        for key in ["api_key_header", "verify_path"] {
            if let Some(v) = c.get(key) {
                ensure!(v.is_string(), "HTTP settings must be strings");
            }
        }
        ensure!(
            c.as_object().is_some_and(|m| m.keys().all(|k| [
                "origin",
                "api_key_header",
                "verify_path"
            ]
            .contains(&k.as_str()))),
            "Unknown HTTP connection setting"
        );
        ensure!(
            s.is_null()
                || s.as_object().is_some_and(|m| m
                    .keys()
                    .all(|k| ["bearer", "api_key"].contains(&k.as_str()))),
            "Unknown HTTP credential field"
        );
        if let Some(name) = c["api_key_header"].as_str() {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())?;
            ensure!(
                ![
                    "host",
                    "cookie",
                    "authorization",
                    "proxy-authorization",
                    "content-length",
                    "transfer-encoding"
                ]
                .contains(&name.as_str()),
                "Restricted credential header"
            );
        }
        Ok(())
    }
    async fn verify(&self, c: &Value, s: &Value) -> Result<()> {
        self.validate(c, s)?;
        let ctx = Invocation {
            tenant: String::new(),
            run: String::new(),
            step: String::new(),
            key: String::new(),
            config: c.clone(),
            secrets: s.clone(),
        };
        self.request(
            &ctx,
            "get",
            &json!({"path":c["verify_path"].as_str().unwrap_or("/")}),
        )
        .await?;
        Ok(())
    }
    async fn execute(&self, c: &Invocation, a: &str, input: Value) -> Result<Value> {
        ensure!(["get", "send"].contains(&a), "Unknown action");
        self.request(c, a, &input).await
    }
}
