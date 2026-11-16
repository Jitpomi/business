//! Process TLS configuration shared by application and standalone plugin entrypoints.
//!
//! reqwest uses rustls-no-provider. Do not rely on a database connection or a
//! different plugin to configure its cryptography as an incidental side effect.
//! An embedding host's explicitly installed provider remains authoritative.
pub fn initialize() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
