use super::*;
#[tokio::test]
#[ignore = "Live Auth0 browser proofs plus development TypeDB; generated database only"]
async fn browser_auth0_proofs_enforce_native_multitenant_policies() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development credentials required"
    );
    let root = std::path::PathBuf::from(std::env::var("AUTH0_TEST_PROOFS_DIR")?);
    let proof = |name: &str| -> Result<String> {
        Ok(
            serde_json::from_slice::<Value>(&std::fs::read(root.join(name))?)?["access_token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing browser proof"))?
                .to_owned(),
        )
    };
    let owner = proof("synthetic-owner-proof.json")?;
    let outsider = proof("synthetic-outsider-proof.json")?;
    let config: Vec<Config> = serde_json::from_str(include_str!(
        "../../../deploy/auth0/development-provider.json"
    ))?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let plugin = Arc::new(Oidc::new(config[0].clone())?);
    let verified_owner = plugin.verify(&owner).await?;
    let verified_outsider = plugin.verify(&outsider).await?;
    ensure!(
        verified_owner.subject != verified_outsider.subject
            && verified_owner.amr.contains(&"pwd".into()),
        "Independent browser authentication required"
    );
    let mut wrong = config[0].clone();
    wrong.audience = "frontend-client-id".into();
    ensure!(
        Oidc::new(wrong)?.verify(&owner).await.is_err(),
        "Wrong audience accepted"
    );
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    server::database::install(&driver, &db).await?;
    let outcome=async {
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;tx.query(include_str!("../backend/fixtures.tql")).await?;
        for (identity,v) in [("alice",&verified_owner),("dave",&verified_outsider)] {
            let q=|s:&str|serde_json::to_string(s).unwrap();
            tx.query(format!("match $p isa identity,has identity_id {};insert (identity:$p) isa external_identity,has auth_binding_key {},has auth_issuer {},has auth_subject {};",q(identity),q(&server::auth::binding_key(&v.issuer,&v.subject)),q(&v.issuer),q(&v.subject))).await?;
        }tx.commit().await?;
        let mut registry=server::auth::Registry::default();registry.register(plugin)?;
        let state=Arc::new(server::typedb::TypeDBState{driver:driver.clone(),database:db.clone()});
        let (_,http)=server::app::build_with_authentication(state,"unused-signing-key-only-auth0-is-installed".into(),Arc::new(server::finance::TenantRegistry::default()),Arc::new(registry))?;let app=server::app::router(http);
        let (status,login)=request(&app,"tenant-a","","authentication",json!({"strategy":"jwt","accessToken":owner})).await?;
        ensure!(status==200 && login["user"]["id"]=="alice","Live Auth0 proof did not resolve stable identity: {status}");
        let query=json!({"query":"match $r isa resource;fetch {'id':$r.resource_id};"});
        let (status,data)=request(&app,"tenant-a",&owner,"read",query.clone()).await?;
        ensure!(status==200 && data.to_string().contains("files") && !data.to_string().contains("foreign"),"Tenant A read isolation failed");
        let (status,data)=request(&app,"tenant-b",&outsider,"read",query.clone()).await?;
        ensure!(status==200 && data.to_string().contains("foreign") && !data.to_string().contains("files"),"Tenant B read isolation failed");
        for (tenant,token) in [("tenant-a",&outsider),("tenant-b",&owner)] {
            ensure!(request(&app,tenant,token,"read",query.clone()).await?.0==401,"Cross-tenant membership bypass");
        }
        let source=include_str!("../../public/queries/data/create-resource.tql");
        let write=source.replace("@@identity_id@@","'alice'").replace("@@tenant_id@@","'tenant-a'").replace("@@resource_id@@","'auth0-created'").replace("@@name@@","'Auth0 verified resource'").replace("@@revision@@","'auth0-write'");
        let (status,data)=request(&app,"tenant-a",&owner,"write",json!({"query":write})).await?;ensure!(status==200 && data.to_string().contains("auth0-created"),"Authorized write failed: {status}");
        ensure!(request(&app,"tenant-a",&outsider,"write",json!({"query":write})).await?.0==401,"Other tenant wrote data");
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        tx.query("match $p isa person,has identity_id 'alice';$m isa membership,links(member:$p);update $m has end_at 2000-01-01T00:00:00;").await?;tx.commit().await?;
        ensure!(request(&app,"tenant-a",&owner,"read",query).await?.0==401,"Valid provider token bypassed ended native membership");
        Ok::<_,anyhow::Error>(())
    }.await;
    driver.databases().get(&db).await?.delete().await?;
    outcome
}
