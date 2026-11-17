//! Explicit loopback-only fixture transport. Never selected on connection failure.
//! Hosted acceptance keeps certificate validation and the caller's credentials.
pub async fn connect(
    address: &str,
    username: &str,
    password: &str,
    tls: bool,
) -> anyhow::Result<typedb_driver::TypeDBDriver> {
    server::tls::initialize();
    let local = std::env::var("BUSINESS_TEST_LOCAL_ONLY").as_deref() == Ok("true");
    let tls = if local {
        let uri = reqwest::Url::parse(address)?;
        anyhow::ensure!(
            matches!(uri.host_str(), Some("127.0.0.1" | "[::1]"))
                && uri.port().is_some_and(|p| p >= 1024),
            "Local fixture flag cannot disable remote TLS"
        );
        false
    } else {
        tls
    };
    dog_typedb::TypeDBDriverFactory::connect(address, username, password, tls).await
}
