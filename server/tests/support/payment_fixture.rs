/// Complete pre-authorized facts for non-onboarding execution contract fixtures.
/// The real Stripe onboarding acceptance creates its grants through /data instead.
pub fn payment_grant_fixture(customer: &str, plugin: &str, method: &str, revision: &str) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap();
    let now = chrono::Utc::now();
    let at = now.naive_utc().format("%Y-%m-%dT%H:%M:%S%.6f");
    let epoch = now.timestamp();
    format!("$grant_actor isa person,has identity_id {},has email {email},has name 'Synthetic buyer grant',has status 'active';(tenant:$buyer,member:$grant_actor) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;$saved isa bill_saved_method,has bill_key {},has method_ref {},has method_plugin {},has method_automatic true,has method_pending false,has method_active true,has method_verified_at {at};(customer:$c,method:$saved) isa bill_method_owner;$grant isa bill_consent_evidence,has bill_key {},has bill_revision {},has bill_updated {epoch},has consent_claim {},has method_ref {},has method_plugin {},has consent_at {at};(customer:$c,actor:$grant_actor,evidence:$grant) isa bill_consent_owner;",q(customer),q(&format!("fixture-method-{customer}")),q(method),q(plugin),q(&format!("fixture-grant-{customer}")),q(revision),q(&format!("{customer}:{revision}")),q(method),q(plugin),email=q(&format!("{customer}@example.invalid")))
}
