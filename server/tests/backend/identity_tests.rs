//! Real TypeDB and HTTP proofs of independent identities and credentials.
use anyhow::{ensure, Result};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use typedb_driver::{TransactionType, TypeDBDriver};
async fn driver() -> Result<Arc<TypeDBDriver>> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    super::support::auth()?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Disposable development tests only"
    );
    Ok(Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    ))
}
async fn call(
    app: &axum::Router,
    tenant: &str,
    path: &str,
    token: &str,
    body: Value,
) -> Result<(u16, Value)> {
    let mut req = Request::post(path)
        .header("content-type", "application/json")
        .header("x-tenant-id", tenant);
    if !token.is_empty() {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    if path == "/data" {
        req = req.header("x-service-method", "read");
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string()))?)
        .await?;
    let status = response.status().as_u16();
    Ok((
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await?)?,
    ))
}
#[tokio::test]
#[ignore = "Real development TypeDB, generated disposable database; no external providers"]
async fn identities_support_independent_credentials_and_sessions() -> Result<()> {
    let driver = driver().await?;
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    server::database::install(&driver, &db).await?;
    let result=async {
        let hash=bcrypt::hash("rebuild-test-password",4)?;
        super::seed_accounts(&db)?;
        let store=server::auth::plugins::local::Local::configured(&db)?;
        store.import("bob-second","bob-second@example.invalid",&hash,"second")?;
        let second_link=server::auth::local_link("$bob","bob-second")?;
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query(include_str!("fixtures.tql")).await?;
        tx.query(format!(r#"match $bob isa person,has identity_id "bob";$t isa tenant,has tenant_id "tenant-a";insert
          $contact isa person,has identity_id "contact",has name "Contact without login",has status "active";
          $contact2 isa person,has identity_id "contact2",has name "Contact with email",has email "contact@example.invalid",has status "active";
          (tenant:$t,member:$contact) isa membership,has membership_role "member",has start_at 2020-01-01T00:00:00;
          {second_link}"#)).await?;
        tx.commit().await?;
        let state=Arc::new(server::typedb::TypeDBState{driver:driver.clone(),database:db.clone()});
        let (_,http)=server::app::build_with_state(state,"identity-credential-test-secret-at-least-32-bytes".into())?;
        let app=server::app::router(http);
        let query=json!({"query":"match $r isa resource;fetch {'id':$r.resource_id};"});
        let mut tokens=Vec::new();
        for email in ["bob@example.com","bob-second@example.invalid"] {
            let (status,login)=call(&app,"tenant-a","/authentication","",json!({"strategy":"local","email":email,"password":"rebuild-test-password"})).await?;
            ensure!(status==200,"Credential login failed: {status}");
            ensure!(login["user"]["id"]=="bob","Credential changed identity");
            ensure!(!login.to_string().contains(&hash),"Password hash exposed");
            let token=login["accessToken"].as_str().unwrap().to_owned();
            ensure!(call(&app,"tenant-a","/data",&token,query.clone()).await?.0==200,"Session failed");
            ensure!(call(&app,"tenant-b","/data",&token,query.clone()).await?.0==401,"Credential bypassed membership");
            for secret in ["match $c isa external_identity;fetch {'id':$c.auth_subject};","match $c isa external_identity;fetch {'issuer':$c.auth_issuer};","match $c isa external_identity,links(identity:$p);fetch {'person':$p.identity_id};"] {
                let (s,v)=call(&app,"tenant-a","/data",&token,json!({"query":secret})).await?;
                ensure!(s>=400 || v["ok"]["answers"].as_array().is_some_and(Vec::is_empty),"Credential records exposed");
            }
            tokens.push(token);
        }
        ensure!(tokens[0]!=tokens[1],"Credentials shared a session");
        for attack in [
            "match $c isa external_identity;delete $c;fetch {'deleted':true};",
            "match $p isa person,has identity_id 'bob';insert $c isa external_identity,links(identity:$p),has auth_binding_key 'injected',has auth_issuer 'forged',has auth_subject 'forged';fetch {'created':true};",
        ] {
            let request=Request::post("/data").header("content-type","application/json").header("x-tenant-id","tenant-a").header("x-service-method","write").header("authorization",format!("Bearer {}",tokens[0])).body(Body::from(json!({"query":attack}).to_string()))?;
            let _=app.clone().oneshot(request).await?;
            let tx=driver.transaction(&db,TransactionType::Read).await?;
            let intact=server::database::documents(&tx,"match $p isa person,has identity_id 'bob';$c isa external_identity,links(identity:$p);fetch {'subject':$c.auth_subject};").await?;
            ensure!(intact.len()==2 && intact.iter().all(|c|c["subject"]=="bob" || c["subject"]=="bob-second"),"Public query changed identity links");tx.close().await?;
        }
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query("match $p isa person,has identity_id 'bob';$t isa tenant,has tenant_id 'tenant-b';insert (tenant:$t,member:$p) isa membership,has membership_role 'member',has start_at 2020-01-01T00:00:00;").await?;tx.commit().await?;
        for token in &tokens {ensure!(call(&app,"tenant-b","/data",token,query.clone()).await?.0==200,"Another credential could not use the person's second membership");}
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query("match $p isa person,has identity_id 'bob';$t isa tenant,has tenant_id 'tenant-b';$m isa membership,links(tenant:$t,member:$p);update $m has end_at 2000-01-01T00:00:00;").await?;tx.commit().await?;
        for token in &tokens {ensure!(call(&app,"tenant-b","/data",token,query.clone()).await?.0==401,"Ended membership accepted through another credential");}
        // Contact details can change independently of the authenticating address.
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query("match $p isa person,has identity_id 'bob';update $p has email 'bob-contact@example.invalid';").await?;tx.commit().await?;
        ensure!(call(&app,"tenant-a","/authentication","",json!({"strategy":"local","email":"bob@example.com","password":"rebuild-test-password"})).await?.0==200,"Contact edit changed login");
        ensure!(store.import("duplicate-login","bob@example.com",&hash,"duplicate").is_err(),"Login attached to two accounts");
        store.rotate("bob",&hash,"rotated")?;
        ensure!(call(&app,"tenant-a","/data",&tokens[0],query.clone()).await?.0==401,"Rotated credential session survived");
        ensure!(call(&app,"tenant-a","/data",&tokens[1],query.clone()).await?.0==200,"Rotation affected another credential");
        store.remove("bob-second")?;
        ensure!(call(&app,"tenant-a","/data",&tokens[1],query).await?.0==401,"Removed credential session survived");
        let tx=driver.transaction(&db,TransactionType::Read).await?;
        let people=server::database::documents(&tx,"match $p isa person,has identity_id 'bob';$m isa membership,links(member:$p);fetch {'id':$p.identity_id};").await?;
        ensure!(!people.is_empty(),"Removing credentials removed person or memberships");tx.close().await?;
        Ok::<_,anyhow::Error>(())
    }.await;
    driver.databases().get(&db).await?.delete().await?;
    result
}
#[tokio::test]
#[ignore = "Real development TypeDB; migrates a generated legacy database only"]
async fn credential_migration_preserves_identity_and_passwords() -> Result<()> {
    migrate_fixture(false).await
}

#[tokio::test]
#[ignore = "Real development TypeDB, separated-credential migration in disposable database"]
async fn separated_credentials_and_pending_recovery_leave_the_graph() -> Result<()> {
    migrate_fixture(true).await
}

async fn migrate_fixture(separated: bool) -> Result<()> {
    let driver = driver().await?;
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    driver.databases().create(&db).await?;
    let result=async {
        let tx=driver.transaction(&db,TransactionType::Schema).await?;
        for source in [if separated {include_str!("credential-identity-schema.tql")}else{include_str!("legacy-identity-schema.tql")},server::database::FUNCTIONS,if separated {include_str!("credential-identity-policy.tql")}else{include_str!("legacy-identity-policy.tql")},server::database::BILLING,if separated{include_str!("credential-account-links.tql")}else{server::database::ACCOUNT_LINKS}] {tx.query(source).await?;}
        tx.commit().await?;
        let hash=bcrypt::hash("preserved-test-password",4)?;
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        let credential=if separated{format!("(person:$p) isa password_credential,has credential_id 'preserved-person',has login_email 'preserved@example.invalid',has password_hash '{hash}',has credential_version 'preserved-version';")}else{String::new()};
        let attributes=if separated{String::new()}else{format!(",has password_hash '{hash}',has credential_version 'preserved-version'")};
        tx.query(format!(r#"insert $p isa person,has identity_id "preserved-person",has name "Preserved",has email "preserved@example.invalid",has status "active"{attributes};{credential}$t isa tenant,has tenant_id "preserved-tenant",has name "Preserved",has status "active",has revision "initial";(tenant:$t,member:$p) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;"#)).await?;tx.commit().await?;
        if separated {
            let tx=driver.transaction(&db,TransactionType::Write).await?;
            tx.query(format!("insert $l isa account_link,has token_key 'pending-reset',has email 'preserved@example.invalid',has link_kind 'reset',has link_target 'preserved-person',has link_version 'preserved-version',has link_expires {},has link_used false;$r isa auth_revocation,has token_key 'pending-revocation';",chrono::Utc::now().timestamp()+1800)).await?;tx.commit().await?;
        }
        let before=server::database::snapshot::records(&driver,&db).await?;
        ensure!(server::credential_migration::migrate(&driver,&db).await?==1,"Wrong migration count");
        ensure!(server::credential_migration::migrate(&driver,&db).await?==0,"Migration is not repeatable");
        let after=server::database::snapshot::records(&driver,&db).await?;
        ensure!(after.len()==if separated{before.len()-2}else{before.len()+1},"Migration changed unrelated record counts");
        for (iid,original) in &before {
            if separated && (original.get("credential_id").is_some() || original.get("token_key").is_some()){continue;}
            let current=after.get(iid).ok_or_else(||anyhow::anyhow!("Migration removed an existing record"))?;
            for (attribute,value) in original.as_object().unwrap() {
                if !matches!(attribute.as_str(),"password_hash"|"credential_version") {
                    ensure!(&current[attribute]==value,"Migration changed an existing business attribute");
                }
            }
        }
        let tx=driver.transaction(&db,TransactionType::Read).await?;
        let rows=server::database::documents(&tx,"match $p isa person,has identity_id 'preserved-person';$c isa external_identity,links(identity:$p);$t isa tenant,has tenant_id 'preserved-tenant';$m isa membership,links(tenant:$t,member:$p);fetch {'subject':$c.auth_subject,'role':$m.membership_role};").await?;
        ensure!(rows.len()==1 && rows[0]["subject"]=="preserved-person" && rows[0]["role"]=="owner","Migration lost identity or membership");tx.close().await?;
        if separated {let store=server::auth::plugins::local::Local::configured(&db)?;ensure!(store.challenge("pending-reset")?.is_some() && store.revoked("pending-revocation")?,"Pending recovery or revocation lost");}
        let external=server::auth::plugins::local::Local::configured(&db)?.lookup("preserved-person",false)?.unwrap();
        ensure!(external["password"]==hash && external["version"]=="preserved-version","Migration lost password or version");
        let schema=driver.databases().get(&db).await?.schema().await?;
        ensure!(!schema.contains("password_hash") && !schema.contains("credential_version") && !schema.contains("auth_revocation") && !schema.contains("account_link"),"Credentials remained in TypeDB");
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query("insert $p isa person,has identity_id 'no-login',has name 'No login',has status 'active';").await?;tx.commit().await?;
        let state=Arc::new(server::typedb::TypeDBState{driver:driver.clone(),database:db.clone()});
        let (_,http)=server::app::build_with_state(state,"migration-login-test-secret-at-least-32-bytes".into())?;
        let app=server::app::router(http);
        let (status,login)=call(&app,"preserved-tenant","/authentication","",json!({"strategy":"local","email":"preserved@example.invalid","password":"preserved-test-password"})).await?;
        ensure!(status==200 && login["user"]["id"]=="preserved-person","Migrated account cannot authenticate");
        Ok::<_,anyhow::Error>(())
    }.await;
    driver.databases().get(&db).await?.delete().await?;
    result
}
