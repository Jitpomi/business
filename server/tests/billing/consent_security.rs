//! Negative consent proposals use real native policies and public plugin attestation.
use super::sandbox_support::{http, ok, save};
use super::stripe_automation::{grant_template, native, read};
use super::*;
use serde_json::Value;
use std::path::Path;
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}
async fn denied(
    app: &axum::Router,
    state: &TypeDBState,
    tenant: &str,
    token: &str,
    query: String,
) -> Result<()> {
    let before = database::snapshot::graph(&state.driver, &state.database, &[]).await?;
    let (status, body) = http(
        app,
        tenant,
        token,
        "/data",
        json!({"query":query}),
        Some("write"),
    )
    .await?;
    ensure!(
        status == 403
            || (status == 200 && body["ok"]["answers"].as_array().is_some_and(Vec::is_empty)),
        "Native policy did not reject or filter the proposal: {status} {body}"
    );
    ensure!(
        database::snapshot::graph(&state.driver, &state.database, &[]).await? == before,
        "Denied consent partially changed the graph"
    );
    Ok(())
}
pub(super) async fn check(
    app: &axum::Router,
    state: &TypeDBState,
    case: &Value,
    buyer_token: &str,
    seller_token: &str,
    root: &Path,
) -> Result<()> {
    let path = root.join("security-fixture.json");
    let fixture: Value = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path)?)?
    } else {
        let v = json!({"customer":uuid::Uuid::new_v4().to_string(),"plan":uuid::Uuid::new_v4().to_string(),"other_plan":uuid::Uuid::new_v4().to_string(),"grant":uuid::Uuid::new_v4().to_string()});
        save(&path, &v)?;
        v
    };
    let c = fixture["customer"].as_str().unwrap();
    let p = fixture["plan"].as_str().unwrap();
    let other = fixture["other_plan"].as_str().unwrap();
    let grant = fixture["grant"].as_str().unwrap();
    let buyer = case["buyer"].as_str().unwrap();
    let tenant = case["tenant"].as_str().unwrap();
    let method = case["payment_method"].as_str().unwrap();
    let external = case["provider_customer"].as_str().unwrap();
    if read(
        state,
        &format!(
            "match $c isa bill_customer,has bill_key {};fetch {{'id':$c.bill_key}};",
            q(c)
        ),
    )
    .await?
    .is_empty()
    {
        let at = chrono::Utc::now().timestamp() + 86400;
        let bindings = q(&json!({"my-finance":external}).to_string());
        let tx = state
            .driver
            .transaction(&state.database, TransactionType::Write)
            .await?;
        tx.query(format!("match $t isa tenant,has tenant_id {};$b isa tenant,has tenant_id {};insert $c isa bill_customer,has bill_key {},has name 'Synthetic native consent security',has bill_email 'dev@jitpomi.com',has bill_policy 'customer',has bill_choice 'manual',has bill_provider 'my-finance',has bill_bindings {bindings},has bill_method_plugin '',has bill_payment_method '',has bill_consent '',has bill_revision 'security-v1';(merchant:$t,buyer:$b,customer:$c) isa bill_account;$p isa bill_plan,has bill_key {},has name 'Reviewed future security plan',has bill_amount 100,has bill_due_days 0,has bill_anchor {at},has bill_next {at},has bill_sequence 0,has bill_interval 'once',has bill_status 'active';(customer:$c,plan:$p) isa bill_plan_owner;$other isa bill_plan,has bill_key {},has name 'Unapproved future security plan',has bill_amount 100,has bill_due_days 0,has bill_anchor {at},has bill_next {at},has bill_sequence 0,has bill_interval 'once',has bill_status 'active';(customer:$c,plan:$other) isa bill_plan_owner;",q(tenant),q(buyer),q(c),q(p),q(other))).await?;
        tx.commit().await?;
    }
    ensure!(read(state,&format!("match $e isa bill_consent_evidence,has bill_key {};fetch {{'id':$e.bill_key}};",q(grant))).await?.is_empty(),"Security fixture already completed; preserve it and use a new private checkpoint for another destructive negative run");
    let select_plugin=format!("match $c isa bill_customer,has bill_key {};update $c has bill_method_plugin 'my-finance';fetch {{'id':$c.bill_key}};",q(c));
    denied(app, state, buyer, buyer_token, select_plugin.clone()).await?;
    ok(
        app,
        buyer,
        buyer_token,
        "/financial",
        json!({"action":"payment_methods","customer_id":c,"plugin_id":"my-finance"}),
        None,
    )
    .await?;
    native(app, buyer, buyer_token, select_plugin).await?;
    native(app,buyer,buyer_token,format!("match $c isa bill_customer,has bill_key {};update $c has bill_payment_method {};fetch {{'id':$c.bill_key}};",q(c),q(method))).await?;
    let proposal = || grant_template(c, grant, "security-v1", p, "my-finance", method);
    // Expiry/inactive states are controlled test facts. Only the provider-facing
    // public capability can refresh the verification timestamp.
    let method_match=format!("match $c isa bill_customer,has bill_key {}; (customer:$c,method:$m) isa bill_method_owner;",q(c));
    for (field, bad, restore) in [
        ("method_verified_at", "2000-01-01T00:00:00", None),
        ("method_pending", "true", Some("false")),
        ("method_active", "false", Some("true")),
    ] {
        let tx = state
            .driver
            .transaction(&state.database, TransactionType::Write)
            .await?;
        tx.query(format!("{method_match}update $m has {field} {bad};"))
            .await?;
        tx.commit().await?;
        denied(app, state, buyer, buyer_token, proposal()).await?;
        if let Some(value) = restore {
            let tx = state
                .driver
                .transaction(&state.database, TransactionType::Write)
                .await?;
            tx.query(format!("{method_match}update $m has {field} {value};"))
                .await?;
            tx.commit().await?;
        }
        ok(
            app,
            buyer,
            buyer_token,
            "/financial",
            json!({"action":"payment_methods","customer_id":c,"plugin_id":"my-finance"}),
            None,
        )
        .await?;
    }
    for (target, field, bad, restore) in [
        ("bill_customer", "bill_policy", "manual", "customer"),
        ("bill_plan", "bill_status", "paused", "active"),
    ] {
        let id = if target == "bill_customer" { c } else { p };
        let query = |value: &str| {
            format!("match $x isa {target},has bill_key {};update $x has {field} {};fetch {{'id':$x.bill_key}};",q(id),q(value))
        };
        native(app, tenant, seller_token, query(bad)).await?;
        denied(app, state, buyer, buyer_token, proposal()).await?;
        native(app, tenant, seller_token, query(restore)).await?;
    }
    native(app, buyer, buyer_token, proposal()).await?;
    // A buyer cannot append another plan to already recorded consent later.
    denied(app,state,buyer,buyer_token,format!("match $e isa bill_consent_evidence,has bill_key {};$p isa bill_plan,has bill_key {};insert (evidence:$e,plan:$p) isa bill_consent_plan;fetch {{'id':$e.bill_key}};",q(grant),q(other))).await?;
    let mode=read(state,&format!("match $c isa bill_customer,has bill_key {};$p isa bill_plan,has bill_key {};$other isa bill_plan,has bill_key {};fetch {{'approved':bill_plan_has_consent($c,$p),'unapproved':bill_plan_has_consent($c,$other),'legacy_choice':$c.bill_choice,'legacy_consent':$c.bill_consent}};",q(c),q(p),q(other))).await?;
    ensure!(
        mode.len() == 1
            && mode[0]["approved"] == true
            && mode[0]["unapproved"] == false
            && mode[0]["legacy_choice"] == "manual"
            && mode[0]["legacy_consent"] == "",
        "Native authority mismatch: {mode:?}"
    );
    save(
        &root.join("consent-security-report.json"),
        &json!({"passed":true,"missing_attestation":"denied","expired_attestation":"denied","pending_method":"denied","inactive_method":"denied","manual_policy":"denied","paused_plan":"denied","scope_expansion":"denied","every_denial_preserved_complete_graph":true,"valid_grant_via_public_template":true,"provider_calls":"verification only; no invoices, payment attempts or mail"}),
    )?;
    Ok(())
}
