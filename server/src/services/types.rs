use dog_auth::hooks::authenticate::AuthParams;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RestParams {
    #[serde(skip)]
    pub stripe_access:
        Option<std::sync::Arc<super::stripe::auth_stripe::auth_stripe_schema::StripeAccess>>,
    #[serde(skip)]
    pub mercury_access:
        Option<std::sync::Arc<super::mercury::auth_mercury::auth_mercury_schema::MercuryAccess>>,
    // Transport marker protects server-only credential services; query drives provider filters.
    pub provider: String,
    pub query: HashMap<String, String>,
    // Issued afresh by authentication hooks; never accepted from HTTP JSON.
    #[serde(skip)]
    pub schema_policy_scope: Option<std::sync::Arc<dog_typedb::isolation::AuthenticatedScope>>,
}

// Type alias for authentication-enabled REST parameters
pub type BusinessParams = AuthParams<RestParams>;
