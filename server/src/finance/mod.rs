//! Business-owned contracts. Plugins translate external protocols into these types.
use anyhow::{bail, ensure, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default)]
pub struct Capabilities {
    pub idempotent_customers: bool,
    pub automatic_collection: bool,
    pub saved_methods: bool,
    pub payment_history: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InvoiceState {
    Scheduled,
    Open,
    Processing,
    Paid,
    Void,
    Uncollectible,
}
impl InvoiceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Open => "open",
            Self::Processing => "processing",
            Self::Paid => "paid",
            Self::Void => "void",
            Self::Uncollectible => "uncollectible",
        }
    }
}
#[derive(Clone, Debug)]
pub struct InvoiceRequest {
    pub id: String,
    pub customer: String,
    pub automatic: bool,
    pub amount_minor: i64,
    pub currency: String,
    pub description: String,
    pub issued: i64,
    pub due: i64,
}
#[derive(Clone, Debug)]
pub struct InvoiceObservation {
    pub external_id: String,
    pub customer: String,
    pub invoice_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub state: InvoiceState,
    pub payment_url: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct SavedMethod {
    pub id: String,
    pub customer: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub brand: Option<String>,
    pub bank_name: Option<String>,
    pub last4: Option<String>,
    pub exp_month: Option<u64>,
    pub exp_year: Option<u64>,
}
#[async_trait]
pub trait FinancialPlugin: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn ready(&self) -> Result<()>;
    async fn webhook(
        &self,
        _headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> std::result::Result<Option<InvoiceObservation>, WebhookError> {
        Err(WebhookError::Invalid)
    }
    async fn create_customer(&self, key: &str, name: &str, email: &str) -> Result<String>;
    /// Return the external ID before any optional finalization; the caller persists it.
    async fn create_invoice(&self, request: &InvoiceRequest) -> Result<String>;
    async fn finalize_invoice(
        &self,
        request: &InvoiceRequest,
        external: &str,
    ) -> Result<InvoiceObservation>;
    async fn observe_invoice(&self, external: &str) -> Result<InvoiceObservation>;
    async fn collect(&self, _external: &str, _key: &str, _method: &str) -> Result<()> {
        bail!("Automatic collection unsupported")
    }
    async fn methods(&self, _customer: &str) -> Result<Vec<SavedMethod>> {
        bail!("Saved methods unsupported")
    }
    async fn method(&self, _customer: &str, _id: &str) -> Result<SavedMethod> {
        bail!("Saved methods unsupported")
    }
    async fn setup_method(
        &self,
        _customer: &str,
        _local: &str,
        _bank: bool,
        _return_url: &str,
        _key: &str,
    ) -> Result<String> {
        bail!("Saved methods unsupported")
    }
    async fn confirm_method(
        &self,
        _customer: &str,
        _local: &str,
        _session: &str,
    ) -> Result<String> {
        bail!("Saved methods unsupported")
    }
    async fn detach_method(&self, _customer: &str, _method: &str, _key: &str) -> Result<()> {
        bail!("Saved methods unsupported")
    }
    async fn history(&self, _external: &str) -> Result<Value> {
        bail!("Payment history unsupported")
    }
}
#[derive(Clone, Debug)]
pub struct Message {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub key: String,
}
#[async_trait]
pub trait NotificationPlugin: Send + Sync {
    async fn send(&self, message: &Message) -> Result<String>;
}

/// A tenant gets an explicit immutable set of plugin instances. No cross-tenant fallback.
#[derive(Clone)]
pub struct TenantRuntime {
    pub tenant: String,
    pub sandbox: bool,
    pub reminder_days: i64,
    pub default_financial: Option<String>,
    financial: HashMap<String, Arc<dyn FinancialPlugin>>,
    pub notification: Option<Arc<dyn NotificationPlugin>>,
    pub allowed_test_recipients: Vec<String>,
}
impl TenantRuntime {
    pub fn new(tenant: String, sandbox: bool) -> Self {
        Self {
            tenant,
            sandbox,
            reminder_days: 7,
            default_financial: None,
            financial: HashMap::new(),
            notification: None,
            allowed_test_recipients: vec![],
        }
    }
    pub fn register(&mut self, id: &str, plugin: Arc<dyn FinancialPlugin>) -> Result<()> {
        validate_plugin_id(id)?;
        ensure!(
            !self.financial.contains_key(id),
            "Duplicate financial plugin instance"
        );
        self.financial.insert(id.into(), plugin);
        Ok(())
    }
    pub fn financial(&self, id: &str) -> Result<Arc<dyn FinancialPlugin>> {
        self.financial
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Financial plugin not configured for this tenant"))
    }
    pub fn validate_delivery(&self, recipient: &str) -> Result<()> {
        ensure!(
            self.notification.is_some(),
            "Notifications disabled for this tenant"
        );
        if self.sandbox {
            ensure!(
                self.allowed_test_recipients
                    .iter()
                    .any(|v| v.eq_ignore_ascii_case(recipient)),
                "Sandbox recipient is not approved"
            );
        }
        Ok(())
    }
    pub async fn send(&self, message: Message) -> Result<String> {
        self.validate_delivery(&message.to)?;
        self.notification
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Notification plugin missing"))?
            .send(&message)
            .await
    }
}
pub fn validate_plugin_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 100
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid plugin instance ID"
    );
    Ok(())
}
pub fn opaque_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control),
        "Invalid external reference"
    );
    Ok(())
}
#[derive(Clone, Default)]
pub struct TenantRegistry {
    tenants: HashMap<String, Arc<TenantRuntime>>,
    source: Option<Arc<dyn RuntimeSource>>,
}
#[async_trait]
pub trait RuntimeSource: Send + Sync {
    async fn resolve(&self, tenant: &str) -> Result<Arc<TenantRuntime>>;
}
impl TenantRegistry {
    pub fn with_source(mut self, source: Arc<dyn RuntimeSource>) -> Self {
        self.source = Some(source);
        self
    }
    pub async fn resolve(&self, tenant: &str) -> Result<Arc<TenantRuntime>> {
        match &self.source {
            Some(s) => s.resolve(tenant).await,
            None => Ok(self
                .get(tenant)
                .unwrap_or_else(|_| Arc::new(TenantRuntime::new(tenant.into(), true)))),
        }
    }
    pub fn register(&mut self, runtime: TenantRuntime) -> Result<()> {
        ensure!(
            !self.tenants.contains_key(&runtime.tenant),
            "Duplicate tenant runtime"
        );
        self.tenants
            .insert(runtime.tenant.clone(), Arc::new(runtime));
        Ok(())
    }
    pub fn get(&self, tenant: &str) -> Result<Arc<TenantRuntime>> {
        self.tenants
            .get(tenant)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Tenant runtime not configured"))
    }
}

#[derive(Debug)]
pub enum WebhookError {
    Unauthorized,
    Invalid,
    Unavailable,
}
impl std::fmt::Display for WebhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for WebhookError {}
