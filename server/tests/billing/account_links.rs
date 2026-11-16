use anyhow::{ensure, Result};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use serde_json::{json, Value};
use server::billing::engine::Engine;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
const PASSWORD: &str = "sandbox-link-test-password";
const GATEWAY: &str = "sandbox-link-test-gateway-at-least-32-characters";
async fn call(
    r: &axum::Router,
    tenant: &str,
    token: &str,
    path: &str,
    mut data: Value,
) -> Result<(u16, Value)> {
    let mut req = Request::post(path)
        .header("content-type", "application/json")
        .header("x-tenant-id", tenant)
        .header("x-gateway-secret", GATEWAY);
    if path == "/data" {
        req = req.header(
            "x-service-method",
            if data["_write"] == true {
                "write"
            } else {
                "read"
            },
        );
        data.as_object_mut().unwrap().remove("_write");
    }
    if path == "/authentication" && data["operation"] == "invitation" {
        req = req.header("x-service-method", "account");
    }
    if !token.is_empty() {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let response = r
        .clone()
        .oneshot(req.body(Body::from(data.to_string()))?)
        .await?;
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), 1_000_000).await?;
    Ok((status, serde_json::from_slice(&body)?))
}
pub async fn check(e: &Engine) -> Result<()> {
    std::env::set_var("BUSINESS_GATEWAY_SECRET", GATEWAY);
    let hash = bcrypt::hash(PASSWORD, 4)?;
    let id = uuid::Uuid::new_v4();
    let auth_link_0 = server::auth::local_link("$u", &id.to_string())?;
    server::auth::plugins::local::Local::configured(&e.state.database)?.import(
        &id.to_string(),
        "owner-links@example.com",
        &hash.to_string(),
        "fixture",
    )?;
    e.write(format!(r#"{} insert $u isa person,has identity_id "{id}",has email "owner-links@example.com",has name "TEST owner",has status "active"; {auth_link_0}(tenant:$t,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;fetch {{"done":true}};"#,e.prefix())).await?;
    let (_, http) = server::app::build_with_state(
        e.state.clone(),
        "account-links-sandbox-signing-secret-long-enough".into(),
    )?;
    let mut registry = server::finance::TenantRegistry::default();
    registry.register((*e.runtime).clone())?;
    let r = server::app::router(http).merge(server::billing::http::managed_router(
        e.state.clone(),
        std::sync::Arc::new(registry),
        Some(e.merchant.clone()),
    )?);
    let (s, login) = call(
        &r,
        "account",
        "",
        "/authentication",
        json!({"strategy":"account","email":"owner-links@example.com","password":PASSWORD}),
    )
    .await?;
    ensure!(s == 200, "Owner login {s}");
    let owner = login["accessToken"].as_str().unwrap();
    let (s, invite) = native_invitation(
        &r,
        &e.merchant,
        owner,
        json!({"action":"invite","name":"TEST invited company","email":"new-links@example.com"}),
    )
    .await?;
    ensure!(s == 200, "Invite failed {s}: {invite}");
    ensure!(
        invite["delivery"]["status"] == "unconfirmed",
        "Unapproved mailbox was sent email"
    );
    let customer = invite["customer_id"].as_str().unwrap();
    let buyer = invite["tenant_id"].as_str().unwrap();
    let (s, renew) = native_invitation(
        &r,
        &e.merchant,
        owner,
        json!({"action":"renew_invite","customer_id":customer}),
    )
    .await?;
    ensure!(s == 200, "Renew failed {s}: {renew}");
    ensure!(
        call(
            &r,
            "account",
            "",
            "/onboarding",
            json!({"action":"accept","token":invite["token"],"password":PASSWORD})
        )
        .await?
        .0 == 400,
        "Old invitation survived renewal"
    );
    let accept = json!({"action":"accept","token":renew["token"],"password":PASSWORD});
    let (s, result) = call(&r, "account", "", "/onboarding", accept.clone()).await?;
    if s != 200 {
        let diagnostic =
            server::auth::AccountLifecycle::new(e.state.clone(), &e.merchant, e.runtime.clone())?
                .public_account_action("invite", &accept)
                .await;
        anyhow::bail!("Accept failed {s}: {result}; {diagnostic:?}");
    }
    ensure!(
        call(&r, "account", "", "/onboarding", accept).await?.0 == 400,
        "Invitation replay accepted"
    );
    let (s, login) = call(
        &r,
        "account",
        "",
        "/authentication",
        json!({"strategy":"account","email":"new-links@example.com","password":PASSWORD}),
    )
    .await?;
    ensure!(s == 200, "New account login failed {s}");
    let buyer_token = login["accessToken"].as_str().unwrap();
    ensure!(
        native_invitation(
            &r,
            &e.merchant,
            buyer_token,
            json!({"action":"invite","name":"denied","email":"denied@example.com"})
        )
        .await?
        .0 >= 400,
        "Buyer invited merchant customer"
    );
    let known = call(
        &r,
        "account",
        "",
        "/recovery",
        json!({"action":"request","email":"new-links@example.com"}),
    )
    .await?;
    let unknown = call(
        &r,
        "account",
        "",
        "/recovery",
        json!({"action":"request","email":"unknown-links@example.com"}),
    )
    .await?;
    ensure!(
        known == unknown && known.0 == 200,
        "Recovery reveals account existence"
    );
    // Known token is seeded only in this disposable database, never an existing account.
    let token = "f".repeat(64);
    let key = format!("{:x}", Sha256::digest(token.as_bytes()));
    let expiry = chrono::Utc::now().timestamp() + 1800;
    let store = server::auth::plugins::local::Local::configured(&e.state.database)?;
    let account = store.lookup("new-links@example.com", true)?.unwrap();
    let target = account["subject"].as_str().unwrap();
    let version = account["version"].as_str().unwrap();
    store.issue(&key,&json!({"kind":"reset","email":"new-links@example.com","target":target,"version":version,"expires":expiry}))?;
    let reset = json!({"action":"reset","token":token,"password":"new-sandbox-link-password","confirm_password":"new-sandbox-link-password"});
    let (a, b) = tokio::join!(
        call(&r, "account", "", "/recovery", reset.clone()),
        call(&r, "account", "", "/recovery", reset)
    );
    let (a, b) = (a?, b?);
    ensure!(
        (a.0 == 200) != (b.0 == 200),
        "Reset race did not have exactly one winner: {a:?} {b:?}"
    );
    ensure!(
        call(
            &r,
            buyer,
            buyer_token,
            "/financial",
            json!({"action":"payment_methods","customer_id":customer})
        )
        .await?
        .0 == 401,
        "Reset did not invalidate old session"
    );
    ensure!(
        call(
            &r,
            "account",
            "",
            "/authentication",
            json!({"strategy":"account","email":"new-links@example.com","password":PASSWORD})
        )
        .await?
        .0 == 401,
        "Old password still works"
    );
    ensure!(call(&r,"account","","/authentication",json!({"strategy":"account","email":"new-links@example.com","password":"new-sandbox-link-password"})).await?.0==200,"New password fails");
    // A second company invitation must verify the existing password, never replace it.
    let (_,existing)=native_invitation(&r,&e.merchant,owner,json!({"action":"invite","name":"TEST existing identity","email":"owner-links@example.com"})).await?;
    ensure!(call(&r,"account","","/onboarding",json!({"action":"accept","token":existing["token"],"password":"wrong-but-long-password"})).await?.0==400,"Existing identity was taken over");
    ensure!(
        call(
            &r,
            "account",
            "",
            "/onboarding",
            json!({"action":"accept","token":existing["token"],"password":PASSWORD})
        )
        .await?
        .0 == 200,
        "Existing identity cannot join"
    );
    // An alternate login belongs to the same person, not a new identity.
    let auth_link_1 = server::auth::local_link("$u", "alternate-owner-login")?;
    server::auth::plugins::local::Local::configured(&e.state.database)?.import(
        "alternate-owner-login",
        "alternate-links@example.com",
        &hash.to_string(),
        "alternate",
    )?;
    e.write(format!(
        r#"{} $u isa person,has identity_id "{id}";insert {auth_link_1}fetch {{"done":true}};"#,
        e.prefix()
    ))
    .await?;
    let (_,alternate)=native_invitation(&r,&e.merchant,owner,json!({"action":"invite","name":"TEST alternate identity","email":"alternate-links@example.com"})).await?;
    let (s, accepted) = call(
        &r,
        "account",
        "",
        "/onboarding",
        json!({"action":"accept","token":alternate["token"],"password":PASSWORD}),
    )
    .await?;
    ensure!(
        s == 200,
        "Alternate login cannot accept invitation: {accepted}"
    );
    let (s, alias) = call(
        &r,
        "account",
        "",
        "/authentication",
        json!({"strategy":"account","email":"alternate-links@example.com","password":PASSWORD}),
    )
    .await?;
    ensure!(
        s == 200 && alias["user"]["id"] == id.to_string(),
        "Alternate login created a new identity"
    );
    // A contact can accept an invitation without replacing their existing identity.
    let contact = uuid::Uuid::new_v4();
    e.write(format!(r#"{} insert $u isa person,has identity_id "{contact}",has name "Existing contact",has email "contact-links@example.com",has status "active";fetch {{"done":true}};"#,e.prefix())).await?;
    let (_,invite_contact)=native_invitation(&r,&e.merchant,owner,json!({"action":"invite","name":"TEST existing contact","email":"contact-links@example.com"})).await?;
    ensure!(
        call(
            &r,
            "account",
            "",
            "/onboarding",
            json!({"action":"accept","token":invite_contact["token"],"password":PASSWORD})
        )
        .await?
        .0 == 200,
        "Existing contact cannot accept invitation"
    );
    let (s, contact_login) = call(
        &r,
        "account",
        "",
        "/authentication",
        json!({"strategy":"account","email":"contact-links@example.com","password":PASSWORD}),
    )
    .await?;
    ensure!(
        s == 200 && contact_login["user"]["id"] == contact.to_string(),
        "Contact identity was replaced"
    );
    let (_, revoked) = native_invitation(
        &r,
        &e.merchant,
        owner,
        json!({"action":"invite","name":"TEST revoked","email":"revoked-links@example.com"}),
    )
    .await?;
    ensure!(
        native_invitation(
            &r,
            &e.merchant,
            owner,
            json!({"action":"revoke_invite","customer_id":revoked["customer_id"]})
        )
        .await?
        .0 == 200,
        "Revocation failed"
    );
    ensure!(
        call(
            &r,
            "account",
            "",
            "/onboarding",
            json!({"action":"accept","token":revoked["token"],"password":PASSWORD})
        )
        .await?
        .0 == 400,
        "Revoked link accepted"
    );
    let (_, expired) = native_invitation(
        &r,
        &e.merchant,
        owner,
        json!({"action":"invite","name":"TEST expired","email":"expired-links@example.com"}),
    )
    .await?;
    e.write(format!(r#"{} $l isa account_invitation,has invitation_id {};update $l has invitation_expires 1,has invitation_valid_until 1970-01-01T00:00:01;fetch {{"done":true}};"#,e.prefix(),expired["invitation_id"])).await?;
    ensure!(
        call(
            &r,
            "account",
            "",
            "/onboarding",
            json!({"action":"accept","token":expired["token"],"password":PASSWORD})
        )
        .await?
        .0 == 400,
        "Expired link accepted"
    );
    let denied = r
        .clone()
        .oneshot(
            Request::post("/recovery")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"action":"request","email":"owner-links@example.com"}).to_string(),
                ))?,
        )
        .await?;
    ensure!(
        denied.status() == 401,
        "Direct recovery bypassed the gateway"
    );
    // An old portal variable must not create a second gateway configuration path.
    std::env::remove_var("BUSINESS_GATEWAY_SECRET");
    std::env::set_var("PORTAL_ORIGIN_SECRET", GATEWAY);
    let legacy = call(
        &r,
        "account",
        "",
        "/recovery",
        json!({"action":"request","email":"owner-links@example.com"}),
    )
    .await;
    std::env::set_var("BUSINESS_GATEWAY_SECRET", GATEWAY);
    std::env::remove_var("PORTAL_ORIGIN_SECRET");
    ensure!(legacy?.0 == 401, "Legacy gateway configuration accepted");
    println!("Verified invitation renewal, acceptance, replay protection, tenant permissions, generic recovery, reset race, password change and session revocation.");
    Ok(())
}

// Integration driver for public TypeQL and authentication plugin requests. This
// intentionally exercises native authorization rather than seeding domain state.
async fn native_invitation(
    r: &axum::Router,
    tenant: &str,
    token: &str,
    data: Value,
) -> Result<(u16, Value)> {
    let q = |v: &str| serde_json::to_string(v).unwrap();
    let revision = uuid::Uuid::new_v4().to_string();
    let invitation = uuid::Uuid::new_v4().to_string();
    let action = data["action"].as_str().unwrap();
    let customer;
    let buyer;
    let mut query;
    if action == "invite" {
        customer = uuid::Uuid::new_v4().to_string();
        buyer = uuid::Uuid::new_v4().to_string();
        let name = data["name"].as_str().unwrap();
        let email = data["email"].as_str().unwrap();
        query=format!("match $t isa tenant,has tenant_id {};put $b isa tenant,has tenant_id {},has name {},has status 'active',has revision {};$c isa bill_customer,has bill_key {},has name {},has bill_email {},has bill_customer_claim {},has bill_policy 'customer',has bill_choice 'manual',has bill_provider 'stripe',has bill_bindings '{{}}',has bill_method_plugin '',has bill_payment_method '',has bill_consent '',has bill_revision {};(merchant:$t,buyer:$b,customer:$c) isa bill_account;",q(tenant),q(&buyer),q(name),q(&revision),q(&customer),q(name),q(email),q(&format!("{tenant}:{email}")),q(&revision));
    } else {
        customer = data["customer_id"].as_str().unwrap().into();
        let(status,out)=call(r,tenant,token,"/data",json!({"query":format!("match $c isa bill_customer,has bill_key {}; (customer:$c,buyer:$b) isa bill_account;$b isa tenant;fetch {{'buyer':$b.tenant_id,'revision':$b.revision}};",q(&customer))})).await?;
        if status != 200 {
            return Ok((status, out));
        }
        let rows = out["ok"]["answers"].as_array().unwrap();
        if rows.len() != 1 {
            return Ok((403, json!({})));
        }
        buyer = rows[0]["data"]["buyer"].as_str().unwrap().into();
        query=format!("match $c isa bill_customer,has bill_key {};$b isa tenant,has tenant_id {},has revision {};(buyer:$b,customer:$c) isa bill_account;update $b has revision {};",q(&customer),q(&buyer),rows[0]["data"]["revision"],q(&revision));
    }
    if action != "revoke_invite" {
        query+=&format!("match $c has bill_email $email_attr;let $email=$email_attr;let $now=request_now();let $until=$now+P2D;put $l isa account_invitation,has invitation_id {},has invitation_target {},has invitation_email == $email,has invitation_revision {},has invitation_expires {},has invitation_valid_until == $until,has invitation_used false;",q(&invitation),q(&buyer),q(&revision),chrono::Utc::now().timestamp()+172800);
    }
    query += "fetch {'id':$c.bill_key};";
    let (status, out) = call(
        r,
        tenant,
        token,
        "/data",
        json!({"query":query,"_write":true}),
    )
    .await?;
    if status != 200 {
        return Ok((status, out));
    }
    if action == "revoke_invite" {
        return Ok((200, json!({"revoked":true})));
    }
    let (status, mut out) = call(
        r,
        tenant,
        token,
        "/authentication",
        json!({"operation":"invitation","invitation_id":invitation}),
    )
    .await?;
    out["customer_id"] = json!(customer);
    out["tenant_id"] = json!(buyer);
    Ok((status, out))
}
