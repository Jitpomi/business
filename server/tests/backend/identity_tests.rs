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
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query(include_str!("fixtures.tql").replace("@@password_hash@@",&hash)).await?;
        tx.query(format!(r#"match $bob isa person,has identity_id "bob";$t isa tenant,has tenant_id "tenant-a";insert
          $contact isa person,has identity_id "contact",has name "Contact without login",has status "active";
          $contact2 isa person,has identity_id "contact2",has name "Contact with email",has email "contact@example.invalid",has status "active";
          (tenant:$t,member:$contact) isa membership,has membership_role "member",has start_at 2020-01-01T00:00:00;
          (person:$bob) isa password_credential,has credential_id "bob-second",has login_email "bob-second@example.invalid",has password_hash "{hash}",has credential_version "second";"#)).await?;
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
            for secret in ["match $c isa authentication_credential;fetch {'id':$c.credential_id};","match $c isa password_credential;fetch {'login':$c.login_email};","match $c isa password_credential,links(person:$p);fetch {'person':$p.identity_id};"] {
                let (s,v)=call(&app,"tenant-a","/data",&token,json!({"query":secret})).await?;
                ensure!(s>=400 || v["ok"]["answers"].as_array().is_some_and(Vec::is_empty),"Credential records exposed");
            }
            tokens.push(token);
        }
        ensure!(tokens[0]!=tokens[1],"Credentials shared a session");
        for attack in [
            "match $c isa password_credential;update $c has credential_version 'stolen';fetch {'changed':true};",
            "match $c isa password_credential;delete $c;fetch {'deleted':true};",
            "match $p isa person,has identity_id 'bob';insert $c isa password_credential,links(person:$p),has credential_id 'injected',has login_email 'injected@example.invalid',has password_hash 'bad',has credential_version 'bad';fetch {'created':true};",
        ] {
            let request=Request::post("/data").header("content-type","application/json").header("x-tenant-id","tenant-a").header("x-service-method","write").header("authorization",format!("Bearer {}",tokens[0])).body(Body::from(json!({"query":attack}).to_string()))?;
            let response=app.clone().oneshot(request).await?;
            let status=response.status();
            let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),1_000_000).await?)?;
            ensure!((status.is_client_error() || status.is_server_error()) || (status.is_success() && body["ok"]["answers"].is_array()),"Credential mutation produced an invalid response: {status} {body}");
            // A constant fetch can return a value even when protected matches have
            // no candidates. Verify the actual stored state, not that projection.
            let tx=driver.transaction(&db,TransactionType::Read).await?;
            let intact=server::database::documents(&tx,"match $p isa person,has identity_id 'bob';$c isa password_credential,links(person:$p);fetch {'id':$c.credential_id,'version':$c.credential_version};").await?;
            ensure!(intact.len()==2 && intact.iter().all(|c|c["version"]=="initial" || c["version"]=="second"),"Public credential mutation changed stored credentials");tx.close().await?;
        }
        let tx=driver.transaction(&db,TransactionType::Read).await?;
        let intact=server::database::documents(&tx,"match $p isa person,has identity_id 'bob';$c isa password_credential,links(person:$p);fetch {'id':$c.credential_id,'version':$c.credential_version};").await?;
        ensure!(intact.len()==2 && intact.iter().all(|c|c["version"]=="initial" || c["version"]=="second"),"Public credential mutation changed stored credentials");tx.close().await?;
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
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        let duplicate=tx.query(format!("match $p isa person,has identity_id 'contact';insert (person:$p) isa password_credential,has credential_id 'duplicate-login',has login_email 'bob@example.com',has password_hash '{hash}',has credential_version 'duplicate';")).await;
        if duplicate.is_ok(){ensure!(tx.commit().await.is_err(),"Login identifier attached to two people");}else{tx.close().await?;}

        ensure!(call(&app,"tenant-a","/authentication","",json!({"strategy":"local","email":"contact@example.invalid","password":"rebuild-test-password"})).await?.0==401,"Contact became a login");
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query("match $c isa password_credential,has credential_id 'bob';update $c has credential_version 'rotated';").await?;tx.commit().await?;
        ensure!(call(&app,"tenant-a","/data",&tokens[0],query.clone()).await?.0==401,"Rotated credential session survived");
        ensure!(call(&app,"tenant-a","/data",&tokens[1],query.clone()).await?.0==200,"Rotation affected another credential");
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query("match $c isa password_credential,has credential_id 'bob-second';delete $c;").await?;tx.commit().await?;
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
    let driver = driver().await?;
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    driver.databases().create(&db).await?;
    let result=async {
        let tx=driver.transaction(&db,TransactionType::Schema).await?;
        for source in [include_str!("legacy-identity-schema.tql"),server::database::FUNCTIONS,include_str!("legacy-identity-policy.tql"),server::database::BILLING,server::database::ACCOUNT_LINKS] {tx.query(source).await?;}
        tx.commit().await?;
        let hash=bcrypt::hash("preserved-test-password",4)?;
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query(format!(r#"insert $p isa person,has identity_id "preserved-person",has name "Preserved",has email "preserved@example.invalid",has status "active",has password_hash "{hash}",has credential_version "preserved-version";$t isa tenant,has tenant_id "preserved-tenant",has name "Preserved",has status "active",has revision "initial";(tenant:$t,member:$p) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;"#)).await?;tx.commit().await?;
        let before=server::database::snapshot::records(&driver,&db).await?;
        ensure!(server::credential_migration::migrate(&driver,&db).await?==1,"Wrong migration count");
        ensure!(server::credential_migration::migrate(&driver,&db).await?==0,"Migration is not repeatable");
        let after=server::database::snapshot::records(&driver,&db).await?;
        ensure!(after.len()==before.len()+1,"Migration changed unrelated record counts");
        for (iid,original) in &before {
            let current=after.get(iid).ok_or_else(||anyhow::anyhow!("Migration removed an existing record"))?;
            for (attribute,value) in original.as_object().unwrap() {
                if !matches!(attribute.as_str(),"password_hash"|"credential_version") {
                    ensure!(&current[attribute]==value,"Migration changed an existing business attribute");
                }
            }
        }
        let tx=driver.transaction(&db,TransactionType::Read).await?;
        let rows=server::database::documents(&tx,"match $p isa person,has identity_id 'preserved-person';$c isa password_credential,links(person:$p);$t isa tenant,has tenant_id 'preserved-tenant';$m isa membership,links(tenant:$t,member:$p);fetch {'id':$c.credential_id,'email':$c.login_email,'hash':$c.password_hash,'version':$c.credential_version,'role':$m.membership_role};").await?;
        ensure!(rows.len()==1 && rows[0]["hash"]==hash && rows[0]["version"]=="preserved-version" && rows[0]["role"]=="owner" && rows[0]["id"]=="preserved-person" && rows[0]["email"]=="preserved@example.invalid","Migration lost existing work");tx.close().await?;
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
