use crate::services::BusinessParams;
use anyhow::Result;
use dog_core::DogAppBuilder;
use serde_json::Value;
use std::env;

pub fn config(app: &mut DogAppBuilder<Value, BusinessParams>) -> Result<()> {
    validate_security(
        &env::var("AUTH_JWT_SECRET").unwrap_or_default(),
        &env::var("TYPEDB_TLS").unwrap_or_else(|_| "true".into()),
        &env::var("TYPEDB_FORCE_RECREATE").unwrap_or_else(|_| "false".into()),
        &env::var("TYPEDB_USERNAME").unwrap_or_default(),
    )?;
    crate::admission::configured_limit()?;
    config_http(app)?;
    config_typedb(app)?;
    app.set("auth.jwt.secret", env::var("AUTH_JWT_SECRET")?);
    Ok(())
}

fn config_http(app: &mut DogAppBuilder<Value, BusinessParams>) -> Result<()> {
    let host = env::var("HTTP_HOST")?;
    let port = env::var("HTTP_PORT")?;
    app.set("http.host", host);
    app.set("http.port", port);
    Ok(())
}

fn config_typedb(app: &mut DogAppBuilder<Value, BusinessParams>) -> Result<()> {
    let addr = env::var("TYPEDB_ADDR")?;
    let db = env::var("TYPEDB_DB")?;
    let username = env::var("TYPEDB_USERNAME")?;
    let password = env::var("TYPEDB_PASSWORD")?;
    app.set("typedb.addr", addr);
    app.set("typedb.db", db);
    app.set("typedb.username", username);
    app.set("typedb.password", password);
    Ok(())
}

fn validate_security(secret: &str, tls: &str, reset: &str, user: &str) -> Result<()> {
    anyhow::ensure!(
        secret.len() >= 32 && secret.trim().len() >= 32,
        "A nonblank JWT secret of at least 32 bytes is required"
    );
    anyhow::ensure!(tls == "true", "TLS is required");
    anyhow::ensure!(reset == "false", "Startup reset is forbidden");
    anyhow::ensure!(
        !user.is_empty() && user != "admin",
        "A non-admin runtime account is required"
    );
    Ok(())
}
#[cfg(test)]
#[path = "../tests/unit/config/tests.rs"]
mod tests;
