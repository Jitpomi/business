//! Business-owned contracts. Plugins translate external protocols into these types.
use anyhow::{bail, ensure, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Capabilities {
    pub idempotent_customers: bool,
    pub automatic_collection: bool,
    pub saved_methods: bool,
    pub payment_history: bool,
    pub hosted_checkout: bool,
    pub refunds: bool,
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RefundState {
    Pending,
    Succeeded,
    Failed,
}
impl RefundState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}
/// The plugin proved that submission never happened. Timeouts and unknown acknowledgements
/// must not use this error; they remain reserved until read-only reconciliation succeeds.
#[derive(Debug)]
pub struct RefundNotSubmitted;
impl std::fmt::Display for RefundNotSubmitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The provider did not submit this refund")
    }
}
impl std::error::Error for RefundNotSubmitted {}
#[derive(Clone, Debug, serde::Serialize)]
pub struct RefundObservation {
    pub external_id: String,
    pub request_id: String,
    pub invoice_id: String,
    pub customer: String,
    pub amount_minor: i64,
    pub currency: String,
    pub state: RefundState,
}
#[async_trait]
pub trait FinancialPlugin: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn brand(&self) -> Option<&'static str> {
        None
    }
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
    async fn refund(
        &self,
        _invoice: &str,
        _request_id: &str,
        _amount: i64,
    ) -> Result<RefundObservation> {
        Err(
            dog_core::DogError::bad_request("Refunds are unsupported by this financial plugin")
                .into_anyhow(),
        )
    }
    /// Read-only recovery by immutable request reference. Absence is never permission to replay.
    async fn find_refund(
        &self,
        _invoice: &str,
        _request_id: &str,
    ) -> Result<Option<RefundObservation>> {
        Err(dog_core::DogError::bad_request(
            "Refund reconciliation is unsupported by this financial plugin",
        )
        .into_anyhow())
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
    pub fn checkout_plugins(&self) -> Vec<serde_json::Value> {
        let mut plugins: Vec<_> = self.financial.iter().map(|(id,p)| serde_json::json!({"id":id,"capabilities":p.capabilities(),"brand":p.brand(),"sandbox":self.sandbox})).collect();
        plugins.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        plugins
    }
    pub fn financial(&self, id: &str) -> Result<Arc<dyn FinancialPlugin>> {
        self.financial.get(id).cloned().ok_or_else(|| {
            dog_core::DogError::conflict("Financial system is not connected for this company")
                .into_anyhow()
        })
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
/// Safe configuration projection; reading account context never opens credentials.
#[derive(Clone, serde::Serialize)]
pub struct Configuration {
    pub sandbox: bool,
    pub default_financial: Option<String>,
}
impl From<&TenantRuntime> for Configuration {
    fn from(runtime: &TenantRuntime) -> Self {
        Self {
            sandbox: runtime.sandbox,
            default_financial: runtime.default_financial.clone(),
        }
    }
}
#[async_trait]
pub trait RuntimeSource: Send + Sync {
    async fn resolve(&self, tenant: &str) -> Result<Arc<TenantRuntime>>;
    async fn configuration(&self, tenant: &str) -> Result<Configuration> {
        Ok(Configuration::from(self.resolve(tenant).await?.as_ref()))
    }
}
impl TenantRegistry {
    pub async fn configuration(&self, tenant: &str) -> Result<Configuration> {
        match &self.source {
            Some(source) => source.configuration(tenant).await,
            None => Ok(Configuration::from(self.resolve(tenant).await?.as_ref())),
        }
    }
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
        self.tenants.get(tenant).cloned().ok_or_else(|| {
            dog_core::DogError::conflict("Financial systems are not configured for this company")
                .into_anyhow()
        })
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
