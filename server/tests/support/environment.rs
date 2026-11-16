//! Restore process settings even when an explicitly serial acceptance test fails.
//! These suites must run with --test-threads=1, as documented in tests/README.md.
pub struct Environment(Vec<(String, Option<std::ffi::OsString>)>);
impl Environment {
    pub fn capture(keys: &[&str]) -> Self {
        Self(
            keys.iter()
                .map(|key| ((*key).to_owned(), std::env::var_os(key)))
                .collect(),
        )
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, previous) in &self.0 {
            match previous {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
