//! Explicit, private, process-local account storage for integration tests.
pub fn auth() -> anyhow::Result<()> {
    // The HTTP test client enables rustls-no-provider. Its process must choose
    // its own provider, independently of the database driver's TLS internals.
    server::tls::initialize();
    static DIRECTORY: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    if DIRECTORY.get().is_none() {
        let dir = tempfile::tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let _ = DIRECTORY.set(dir);
    }
    let secrets = DIRECTORY.get().unwrap().path().join("secrets");
    if !secrets.exists() {
        std::fs::create_dir(&secrets)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    std::env::set_var("AUTOMATION_SECRET_STORE_DIR", secrets);
    std::env::set_var("AUTH_LOCAL_ENABLED", "true");
    std::env::set_var("AUTH_LOCAL_STORE_DIR", DIRECTORY.get().unwrap().path());
    Ok(())
}
