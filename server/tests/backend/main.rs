//! Public API acceptance tests for frontend TypeQL, authentication and tenant policy.
use server::app::{build_with_state, router};
use server::database::*;

mod domain_model_tests;
mod provisioning_tests;
mod review_tests;
mod schema_policy_tests;

mod identity_tests;

#[path = "../support/mod.rs"]
mod support;

fn seed_accounts(database: &str) -> anyhow::Result<()> {
    let store = server::auth::plugins::local::Local::configured(database)?;
    let hash = bcrypt::hash("rebuild-test-password", 4)?;
    for name in ["alice", "bob", "carol", "dave"] {
        store.import(name, &format!("{name}@example.com"), &hash, "initial")?;
    }
    Ok(())
}
