use dog_auth::hooks::authenticate::AuthParams;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RestParams {
    #[serde(skip)]
    pub plugin_access: PluginAccess,
    // Transport marker protects server-only credential services; query drives provider filters.
    pub provider: String,
    pub query: HashMap<String, String>,
    // Issued afresh by authentication hooks; never accepted from HTTP JSON.
    #[serde(skip)]
    pub schema_policy_scope: Option<std::sync::Arc<dog_typedb::isolation::AuthenticatedScope>>,
}

// Type alias for authentication-enabled REST parameters
pub type BusinessParams = AuthParams<RestParams>;

/// Type-safe, request-local plugin credentials. Neither serialized nor accepted from clients.
#[derive(Clone, Default)]
pub struct PluginAccess(
    std::collections::HashMap<&'static str, std::sync::Arc<dyn std::any::Any + Send + Sync>>,
);
impl std::fmt::Debug for PluginAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginAccess")
            .field("plugins", &self.0.keys())
            .finish_non_exhaustive()
    }
}
impl PluginAccess {
    pub fn set<T: std::any::Any + Send + Sync>(
        &mut self,
        key: &'static str,
        value: std::sync::Arc<T>,
    ) {
        self.0.insert(key, value);
    }
    pub fn get<T: std::any::Any>(&self, key: &str) -> Option<&T> {
        self.0.get(key)?.downcast_ref()
    }
    pub fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }
}

pub fn actor(ctx: &dog_core::TenantContext, p: &BusinessParams) -> anyhow::Result<String> {
    let u = p
        .auth_result
        .as_ref()
        .and_then(|v| v.get("user"))
        .ok_or_else(|| dog_core::DogError::not_authenticated("Please sign in").into_anyhow())?;
    if !p.authenticated || u["tenant_id"].as_str() != Some(&ctx.tenant_id.0) {
        return Err(dog_core::DogError::not_authenticated("Company access changed").into_anyhow());
    }
    u["id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| {
            dog_core::DogError::not_authenticated("Missing verified identity").into_anyhow()
        })
}
