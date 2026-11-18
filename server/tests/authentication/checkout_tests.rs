//! Signed hosted identities enrol only into their own buyer account.
use super::*;
use anyhow::Context;
#[tokio::test]
#[ignore = "Development TypeDB, disposable checkout identity fixture"]
async fn checkout_identity_enrolment_and_native_tenant_isolation() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    let driver = Arc::new(
        test_driver::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    server::database::install(&driver, &db).await?;
    let result=async {
        let provider=Provider::new().await?;
        let mut plugins=server::auth::Registry::default();plugins.register(Arc::new(provider.plugin("checkout")?))?;
        let merchant=uuid::Uuid::new_v4().to_string();let pricing=uuid::Uuid::new_v4().to_string();
        let now=server::clock::native_utc();
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        tx.query(format!("insert $t isa tenant,has tenant_id '{merchant}',has name 'Shop',has status 'active',has revision 'initial';$p isa commerce_pricing,has cx_id '{pricing}',has cx_created {now},has name 'Public checkout',has cx_tax_bp 0,has cx_discount_bp 0,has cx_shipping 0,has cx_enabled true,has cx_instalments true,has cx_limit 12,has cx_deposit_bp 0,has cx_plan_period 'month',has cx_public true,has cx_plugin 'bank';(merchant:$t,pricing:$p) isa commerce_pricing_owner;$f isa commerce_plugin,has cx_id '{merchant}:bank',has cx_plugin 'bank',has cx_enabled true,has cx_refunds true,has cx_verified {now};(merchant:$t,plugin:$f) isa commerce_plugin_owner;")).await?;tx.commit().await?;
        let state=Arc::new(server::typedb::TypeDBState{driver:driver.clone(),database:db.clone()});
        let (_,http)=server::app::build_with_authentication(state.clone(),"checkout-identity-test-signing-secret-at-least-32-bytes".into(),Arc::new(server::finance::TenantRegistry::default()),Arc::new(plugins))?;
        let app=server::app::router(http);
        async fn enrol(app:&axum::Router,merchant:&str,proof:&str)->Result<(u16,Value)>{
            use tower::ServiceExt;
            let response=app.clone().oneshot(axum::http::Request::post("/authentication").header("content-type","application/json").header("x-service-method","account").body(axum::body::Body::from(json!({"operation":"checkout_enrolment","merchant_id":merchant,"accessToken":proof}).to_string()))?).await?;
            let status=response.status().as_u16();let bytes=axum::body::to_bytes(response.into_body(),1_000_000).await?;Ok((status,serde_json::from_slice(&bytes)?))
        }
        let mut claims=provider.claims();claims["email_verified"]=json!(false);
        ensure!(enrol(&app,&merchant,&provider.token(&claims,"at+jwt")?).await?.0==403,"Unverified contact enrolled");
        claims["email_verified"]=json!(true);let proof=provider.token(&claims,"at+jwt")?;
        let (status,first)=enrol(&app,&merchant,&proof).await?;
        ensure!(status==200,"Enrollment failed: {status} {first}");
        let buyer=first["tenant_id"].as_str().context("Buyer scope")?;
        ensure!(buyer!=merchant);
        ensure!(enrol(&app,&merchant,&proof).await?.1==first,"Repeat created another account");
        let (status,login)=request(&app,buyer,"","authentication",json!({"strategy":"jwt","accessToken":proof})).await?;
        ensure!(status==200,"Email-free domain identity cannot sign in: {status} {login}");
        let person=login["user"]["id"].as_str().context("Person id")?;
        let query=json!({"query":"match $t isa tenant;fetch {'id':$t.tenant_id};"});
        let (status,rows)=request(&app,buyer,&proof,"read",query.clone()).await?;
        ensure!(status==200&&!rows.to_string().contains(&merchant),"Buyer exposed seller tenant");
        ensure!(request(&app,&merchant,&proof,"read",query.clone()).await?.0==401,"Checkout granted merchant membership");
        claims["sub"]=json!("different-subject-same-contact");let other=provider.token(&claims,"at+jwt")?;
        let (status,second)=enrol(&app,&merchant,&other).await?;
        ensure!(status==200&&second["tenant_id"]!=first["tenant_id"],"Contact email merged independent identities");
        claims["sub"]=json!("simultaneous-sign-ins");let concurrent=provider.token(&claims,"at+jwt")?;
        let (left,right)=tokio::join!(enrol(&app,&merchant,&concurrent),enrol(&app,&merchant,&concurrent));
        let (left_status,left)=left?;let(right_status,right)=right?;
        ensure!(left_status==200 && right_status==200 && left==right,"Concurrent sign-ins did not resolve one buyer account: {left_status} {right_status}");
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        tx.query(format!("match $t isa tenant,has tenant_id '{buyer}';$p isa person,has identity_id '{person}';$m isa membership,links(tenant:$t,member:$p);delete $m;")).await?;tx.commit().await?;
        ensure!(enrol(&app,&merchant,&proof).await?.0!=200,"Enrollment restored revoked access");
        claims["sub"]=json!("closed-shop-new-person");let closed=provider.token(&claims,"at+jwt")?;
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        tx.query(format!("match $p isa commerce_pricing,has cx_id '{pricing}';update $p has cx_enabled false;")).await?;tx.commit().await?;
        ensure!(enrol(&app,&merchant,&closed).await?.0!=200,"Closed shop enrolled new customer");
        Ok::<_,anyhow::Error>(())
    }.await;
    if result.is_ok() {
        driver.databases().get(&db).await?.delete().await?;
    } else {
        eprintln!("Retained identity fixture: {db}");
    }
    result
}
