//! Explicit offline provisioning from an operator-verified provider import manifest.
use anyhow::{ensure, Result};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    identity_id: String,
    issuer: String,
    subject: String,
}
#[tokio::main]
async fn main() -> Result<()> {
    ensure!(
        std::env::args().nth(1).as_deref() == Some("--apply"),
        "Explicit --apply required"
    );
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(
        std::env::var("BUSINESS_WRITERS_STOPPED").as_deref() == Ok("true"),
        "Stop writers before provisioning"
    );
    let db = std::env::var("TYPEDB_DB")?;
    server::database::target(&db)?;
    let bindings: Vec<Binding> = serde_json::from_slice(&std::fs::read(std::env::var(
        "AUTH_MIGRATION_BINDINGS_FILE",
    )?)?)?;
    ensure!(
        !bindings.is_empty(),
        "Verified provider import manifest required"
    );
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let tx = driver
        .transaction(&db, typedb_driver::TransactionType::Write)
        .await?;
    let q = |v: &str| serde_json::to_string(v).unwrap();
    for b in &bindings {
        let issuer = reqwest::Url::parse(&b.issuer)?;
        ensure!(
            issuer.scheme() == "https"
                && issuer.username().is_empty()
                && issuer.password().is_none()
                && issuer.query().is_none()
                && issuer.fragment().is_none()
                && !b.subject.is_empty()
                && b.subject.len() <= 255,
            "Invalid provider binding"
        );
        let key = server::auth::binding_key(&b.issuer, &b.subject);
        let existing = server::database::documents(&tx, &format!("match $l isa external_identity,has auth_binding_key {}; $l links (identity:$p); fetch {{'identity':$p.identity_id,'issuer':$l.auth_issuer,'subject':$l.auth_subject}};",q(&key))).await?;
        if !existing.is_empty() {
            ensure!(
                existing.len() == 1
                    && existing[0]["identity"] == b.identity_id
                    && existing[0]["issuer"] == b.issuer
                    && existing[0]["subject"] == b.subject,
                "Provider account already belongs to another identity"
            );
            continue;
        }
        let identity = server::database::documents(
            &tx,
            &format!(
                "match $p isa identity,has identity_id {};fetch {{'id':$p.identity_id}};",
                q(&b.identity_id)
            ),
        )
        .await?;
        ensure!(
            identity.len() == 1,
            "Existing identity required; no identity or membership is created by this tool"
        );
        tx.query(format!("match $p isa identity,has identity_id {};insert (identity:$p) isa external_identity,has auth_binding_key {},has auth_issuer {},has auth_subject {};",q(&b.identity_id),q(&key),q(&b.issuer),q(&b.subject))).await?;
    }
    tx.commit().await?;
    println!("Verified {} provider bindings; existing identities, tenant memberships and permissions retained", bindings.len());
    Ok(())
}
