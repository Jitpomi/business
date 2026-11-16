//! Runs in its own process: no database, finance plugin or prior TLS side effect.
#[test]
fn oidc_constructs_its_http_client_without_a_database_or_payment_plugin() -> anyhow::Result<()> {
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    let config = serde_json::from_value(serde_json::json!({
        "id": "synthetic-oidc",
        "issuer": "https://identity.example.invalid/",
        "audience": "synthetic-api",
        "jwks_uri": "https://identity.example.invalid/jwks"
    }))?;
    let _plugin = server::auth::plugins::oidc::Oidc::new(config)?;
    let selected = rustls::crypto::CryptoProvider::get_default()
        .unwrap()
        .clone();
    server::tls::initialize();
    assert!(std::sync::Arc::ptr_eq(
        &selected,
        rustls::crypto::CryptoProvider::get_default().unwrap()
    ));
    Ok(())
}
