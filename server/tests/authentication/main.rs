//! Real asymmetric token verification, external account persistence and policy contracts.
use anyhow::{ensure, Result};
use base64::Engine;
use serde_json::{json, Value};
use server::auth::{
    plugins::{
        local::Local,
        oidc::{Config, Oidc},
    },
    requirements::Requirement,
    AuthenticationPlugin,
};
use std::sync::Arc;
struct Provider {
    url: String,
    client: reqwest::Client,
    key: jsonwebtoken::EncodingKey,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Provider {
    async fn new() -> Result<Self> {
        let signing = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)?;
        let x = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signing.public_key_raw());
        let jwks = json!({"keys":[{"kty":"OKP","crv":"Ed25519","x":x,"kid":"signing-1","alg":"EdDSA","use":"sig","key_ops":["verify"]}]});
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
        let tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into()),
            )?;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let app = axum::Router::new().route(
            "/jwks",
            axum::routing::get(move || {
                let v = jwks.clone();
                async move { axum::Json(v) }
            }),
        );
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                let app = app.clone();
                tokio::spawn(async move {
                    if let Ok(tls) = acceptor.accept(stream).await {
                        let _ = hyper_util::server::conn::auto::Builder::new(
                            hyper_util::rt::TokioExecutor::new(),
                        )
                        .serve_connection(
                            hyper_util::rt::TokioIo::new(tls),
                            hyper_util::service::TowerToHyperService::new(app),
                        )
                        .await;
                    }
                });
            }
        });
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(reqwest::Certificate::from_der(cert.cert.der())?)
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        Ok(Self {
            url: format!("https://localhost:{port}"),
            client,
            key: jsonwebtoken::EncodingKey::from_ed_pem(signing.serialize_pem().as_bytes())?,
            task,
        })
    }
    fn plugin(&self, id: &str) -> Result<Oidc> {
        Oidc::with_client(
            Config {
                id: id.into(),
                issuer: self.url.clone(),
                audience: "jitpomi-business-api".into(),
                jwks_uri: format!("{}/jwks", self.url),
                account_url: Some(format!("{}/accounts", self.url)),
                token_types: vec!["at+jwt".into()],
                email_claim: "email".into(),
                email_verified_claim: "email_verified".into(),
                amr_claim: "amr".into(),
                auth_time_claim: "auth_time".into(),
                acr_claim: "acr".into(),
                max_token_lifetime_seconds: 900,
            },
            self.client.clone(),
        )
    }
    fn claims(&self) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({"iss":self.url,"sub":"external-user","aud":"jitpomi-business-api","exp":now+300,"iat":now,"auth_time":now,"amr":["mfa","webauthn"],"email":"owner@example.invalid","email_verified":true})
    }
    fn token(&self, claims: &Value, typ: &str) -> Result<String> {
        let mut h = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
        h.kid = Some("signing-1".into());
        h.typ = Some(typ.into());
        Ok(jsonwebtoken::encode(&h, claims, &self.key)?)
    }
}
#[tokio::test]
async fn oidc_accepts_only_trusted_api_tokens_and_signed_assurance() -> Result<()> {
    let host = Provider::new().await?;
    let plugin = host.plugin("enterprise")?;
    let claims = host.claims();
    let token = host.token(&claims, "at+jwt")?;
    let verified = plugin.verify(&token).await?;
    ensure!(
        verified.subject == "external-user" && verified.email_verified,
        "Verified identity lost"
    );
    let strong: Requirement = serde_json::from_value(
        json!({"providers":["enterprise"],"methods":["mfa","webauthn"],"max_age_seconds":60}),
    )?;
    strong.check(&serde_json::to_value(&verified)?)?;
    for (key, value) in [
        ("iss", json!("https://untrusted.invalid")),
        ("aud", json!("frontend-client-id")),
        ("exp", json!(chrono::Utc::now().timestamp() - 60)),
        ("iat", json!(chrono::Utc::now().timestamp() + 600)),
        ("sub", json!("")),
        ("auth_time", json!(chrono::Utc::now().timestamp() + 600)),
    ] {
        let mut bad = claims.clone();
        bad[key] = value;
        ensure!(
            plugin.verify(&host.token(&bad, "at+jwt")?).await.is_err(),
            "Invalid {key} accepted"
        );
    }
    ensure!(
        plugin.verify(&host.token(&claims, "JWT")?).await.is_err(),
        "ID-token type accepted"
    );
    let mut no_time = claims.clone();
    no_time.as_object_mut().unwrap().remove("auth_time");
    let verified = plugin.verify(&host.token(&no_time, "at+jwt")?).await?;
    ensure!(
        strong.check(&serde_json::to_value(verified)?).is_err(),
        "Fresh-login requirement bypassed"
    );
    let mut missing = claims.clone();
    missing.as_object_mut().unwrap().remove("iat");
    ensure!(
        plugin
            .verify(&host.token(&missing, "at+jwt")?)
            .await
            .is_err(),
        "Missing issue time accepted"
    );
    let bad = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"attacker-secret"),
    )?;
    ensure!(
        plugin.verify(&bad).await.is_err(),
        "Symmetric algorithm confusion accepted"
    );
    let account = plugin.account("recover", &json!({})).await?;
    ensure!(
        account["flow"] == "provider" && account["completed"] == false,
        "Account handoff pretended to complete recovery"
    );
    Ok(())
}
#[test]
fn local_store_is_private_durable_and_arbitrates_recovery_and_refresh() -> Result<()> {
    let dir = tempfile::tempdir()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.path().join("accounts.sqlite3");
    let store = Local::open(&path)?;
    store.import("account-a", "first@example.invalid", "hash-one", "v1")?;
    store.import("account-b", "second@example.invalid", "hash-two", "v2")?;
    ensure!(
        store
            .import("account-c", "first@example.invalid", "other", "v3")
            .is_err(),
        "Duplicate login accepted"
    );
    store.issue("reset", &json!({"kind":"reset","target":"account-a"}))?;
    store.reset("reset", "account-a", "v1", "new-hash", "v3")?;
    ensure!(
        store
            .reset("reset", "account-a", "v1", "other", "v4")
            .is_err(),
        "Recovery replay accepted"
    );
    ensure!(
        store.lookup("account-b", false)?.unwrap()["version"] == "v2",
        "Recovery changed another account"
    );
    ensure!(
        store.revoke("refresh-once", i64::MAX)? && !store.revoke("refresh-once", i64::MAX)?,
        "Refresh consumption not one-use"
    );
    drop(store);
    let store = Local::open(&path)?;
    ensure!(
        store.revoked("refresh-once")?
            && store.lookup("account-a", false)?.unwrap()["password"] == "new-hash",
        "Restart lost account state"
    );
    Ok(())
}

async fn request(
    app: &axum::Router,
    tenant: &str,
    token: &str,
    method: &str,
    body: Value,
) -> Result<(u16, Value)> {
    use tower::ServiceExt;
    let mut req = axum::http::Request::post(if method == "authentication" {
        "/authentication"
    } else {
        "/data"
    })
    .header("content-type", "application/json")
    .header("x-tenant-id", tenant);
    if !token.is_empty() {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    if method != "authentication" {
        req = req.header("x-service-method", method);
    }
    let response = app
        .clone()
        .oneshot(req.body(axum::body::Body::from(body.to_string()))?)
        .await?;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}
#[tokio::test]
#[ignore = "Real development TypeDB and local HTTPS IdPs; generated disposable database only"]
async fn oidc_dog_auth_identity_linking_and_tenant_rbac_end_to_end() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development configuration required"
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
    let result=async {
        let primary=Provider::new().await?;let partner=Provider::new().await?;
        let mut registry=server::auth::Registry::default();registry.register(Arc::new(primary.plugin("enterprise")?))?;registry.register(Arc::new(partner.plugin("partner")?))?;
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        tx.query(include_str!("../backend/fixtures.tql")).await?;
        let q=|s:&str|serde_json::to_string(s).unwrap();
        for (person,issuer) in [("bob",&primary.url),("alice",&partner.url)]{
            let key=server::auth::binding_key(issuer,"external-user");
            tx.query(format!("match $p isa person,has identity_id {};insert (identity:$p) isa external_identity,has auth_binding_key {},has auth_issuer {},has auth_subject 'external-user';",q(person),q(&key),q(issuer))).await?;
        }tx.commit().await?;
        std::env::set_var("AUTH_SERVICE_REQUIREMENTS_JSON",json!({"data":{"providers":["enterprise","partner"],"methods":["mfa"],"max_age_seconds":60,"operations":{"write":{"methods":["webauthn"]}}}}).to_string());
        let registry=Arc::new(registry);
        let state=Arc::new(server::typedb::TypeDBState{driver:driver.clone(),database:db.clone()});
        let (_,http)=server::app::build_with_authentication(state.clone(),"oidc-integration-unused-platform-signing-key-long-enough".into(),Arc::new(server::finance::TenantRegistry::default()),registry.clone())?;
        let app=server::app::router(http);
        let token=primary.token(&primary.claims(),"at+jwt")?;
        let (status,login)=request(&app,"tenant-a","","authentication",json!({"strategy":"jwt","accessToken":token})).await?;
        ensure!(status==200 && login["user"]["id"]=="bob","OIDC did not resolve the linked person: {status}");
        let partner_token=partner.token(&partner.claims(),"at+jwt")?;
        let (status,login)=request(&app,"tenant-a","","authentication",json!({"strategy":"jwt","accessToken":partner_token})).await?;
        ensure!(status==200 && login["user"]["id"]=="alice","Same subject from different issuer collided");
        let query=json!({"query":"match $r isa resource;fetch {'id':$r.resource_id};"});
        let (status,rows)=request(&app,"tenant-a",&token,"read",query.clone()).await?;
        ensure!(status==200 && !rows.to_string().contains("foreign"),"Graph tenant policy was bypassed: {status}");
        ensure!(request(&app,"tenant-b",&token,"read",query.clone()).await?.0==401,"External provider bypassed membership");
        let mut weak=primary.claims();weak["amr"]=json!(["pwd"]);let weak_token=primary.token(&weak,"at+jwt")?;
        ensure!(request(&app,"tenant-a",&weak_token,"read",query.clone()).await?.0==403,"Proxy requirement trusted missing MFA");
        weak["amr"]=json!(["mfa"]);let weak_token=primary.token(&weak,"at+jwt")?;
        ensure!(request(&app,"tenant-a",&weak_token,"read",query.clone()).await?.0==200,"Read unnecessarily required write assurance");
        ensure!(request(&app,"tenant-a",&weak_token,"write",query.clone()).await?.0==403,"Per-operation assurance bypassed");
        let mut old=primary.claims();old["auth_time"]=json!(chrono::Utc::now().timestamp()-3600);
        ensure!(request(&app,"tenant-a",&primary.token(&old,"at+jwt")?,"read",query.clone()).await?.0==403,"Recent authentication bypassed");
        // Explicit linking needs both authenticated proofs, never an email match.
        let mut new=partner.claims();new["sub"]=json!("new-external-account");new["email"]=json!("alice@example.com");let new_token=partner.token(&new,"at+jwt")?;
        ensure!(request(&app,"tenant-a","","authentication",json!({"strategy":"jwt","accessToken":new_token})).await?.0==401,"Matching email automatically linked identities");
        use tower::ServiceExt;
        let link=|proof:&str|axum::http::Request::post("/authentication").header("content-type","application/json").header("x-tenant-id","tenant-a").header("x-service-method","account").header("authorization",format!("Bearer {token}")).body(axum::body::Body::from(json!({"plugin":"partner","operation":"link","accessToken":proof}).to_string()));
        ensure!(app.clone().oneshot(link(&partner_token)?).await?.status()==409,"Linking merged two existing people");
        ensure!(app.clone().oneshot(link(&new_token)?).await?.status()==200,"Two-proof account linking failed");
        let (status,login)=request(&app,"tenant-a","","authentication",json!({"strategy":"jwt","accessToken":new_token})).await?;
        ensure!(status==200 && login["user"]["id"]=="bob","Linked account did not retain the person");
        // Machine identities use the same data-layer policy model as people.
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        let machine_key=server::auth::binding_key(&primary.url,"ci-service");
        tx.query(format!("match $bot isa service_account,has identity_id 'bot';insert (identity:$bot) isa external_identity,has auth_binding_key {},has auth_issuer {},has auth_subject 'ci-service';",q(&machine_key),q(&primary.url))).await?;tx.commit().await?;
        std::env::remove_var("AUTH_SERVICE_REQUIREMENTS_JSON");
        let (_,machine_http)=server::app::build_with_authentication(state.clone(),"machine-integration-unused-signing-key-at-least-32-bytes".into(),Arc::new(server::finance::TenantRegistry::default()),registry.clone())?;
        let machine_app=server::app::router(machine_http);let mut machine=primary.claims();machine["sub"]=json!("ci-service");machine["amr"]=json!([]);let machine_token=primary.token(&machine,"at+jwt")?;
        let (status,machine_rows)=request(&machine_app,"tenant-a",&machine_token,"read",query.clone()).await?;
        ensure!(status==200 && machine_rows.to_string().contains("files") && !machine_rows.to_string().contains("foreign"),"Service account did not use native data policies: {status}");
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;tx.query("match $bot isa service_account,has identity_id 'bot';update $bot has status 'suspended';").await?;tx.commit().await?;
        ensure!(request(&machine_app,"tenant-a",&machine_token,"read",query.clone()).await?.0==401,"Suspended service account retained access");
        let schema=driver.databases().get(&db).await?.schema().await?;ensure!(!schema.contains("password_hash") && !schema.contains("credential_version") && !schema.contains("auth_revocation"),"Authentication secrets/state remain in graph");
        Ok::<_,anyhow::Error>(())
    }.await;
    std::env::remove_var("AUTH_SERVICE_REQUIREMENTS_JSON");
    driver.databases().get(&db).await?.delete().await?;
    result
}

#[path = "../support/mod.rs"]
mod support;
#[tokio::test]
#[ignore = "Real development TypeDB; external encrypted credential migration in disposable database"]
async fn encrypted_plugin_credentials_migrate_to_external_references() -> Result<()> {
    support::auth()?;
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development database required"
    );
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    server::database::install(&driver, &db).await?;
    let result=async {
        use aes_gcm::{aead::{Aead,Payload},Aes256Gcm,KeyInit};
        let tenant=uuid::Uuid::new_v4().to_string();let revision=uuid::Uuid::new_v4().to_string();let context=format!("{tenant}/bank/{revision}");
        let secret=json!({"token":"SYNTHETIC-EXTERNAL-CREDENTIAL"});
        let cipher=Aes256Gcm::new_from_slice(&[0u8;32]).map_err(|_|anyhow::anyhow!("Test key failed"))?;
        let nonce=[7u8;12];let ciphertext=cipher.encrypt((&nonce).into(),Payload{msg:secret.to_string().as_bytes(),aad:context.as_bytes()}).map_err(|_|anyhow::anyhow!("Test encryption failed"))?;
        let mut bytes=nonce.to_vec();bytes.extend(ciphertext);let inline=base64::engine::general_purpose::STANDARD.encode(bytes);
        std::env::set_var("AUTOMATION_MASTER_KEY_B64","AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        let legacy=json!({"id":"bank","connector":"http","revision":revision,"config":{"origin":"https://example.com"},"sealed":inline,"enabled":true,"verified":true});
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        tx.query(format!("insert $r isa automation_record,has auto_key '{tenant}/connection/bank',has auto_tenant '{tenant}',has auto_kind 'connection',has auto_payload {},has auto_revision 'original',has auto_status 'enabled',has auto_when 0;",serde_json::to_string(&legacy.to_string())?)).await?;tx.commit().await?;
        ensure!(server::credential_migration::evacuate_plugin_secrets(&driver,&db).await?==1,"Encrypted credential not migrated");
        ensure!(server::credential_migration::evacuate_plugin_secrets(&driver,&db).await?==0,"Migration duplicated credentials");
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Read).await?;
        let rows=server::database::documents(&tx,"match $r isa automation_record;fetch {'payload':$r.auto_payload};").await?;tx.close().await?;
        let payload:Value=serde_json::from_str(rows[0]["payload"].as_str().unwrap())?;
        ensure!(payload.get("sealed").is_none() && !payload.to_string().contains(&inline),"Encrypted credential remained in TypeDB");
        let reference=payload["secret_ref"].as_str().unwrap();ensure!(reference.starts_with("secret:"),"Missing external secret reference");
        let vault=server::automation::vault::Vault::from_env()?;
        ensure!(vault.open(&context,reference)?==secret,"Credential contents changed");
        ensure!(vault.open("another-tenant/bank/revision",reference).is_err(),"Credential binding lost");
        let restarted=server::automation::vault::Vault::from_env()?;ensure!(restarted.open(&context,reference)?==secret,"Restart lost credentials");
        Ok::<_,anyhow::Error>(())
    }.await;
    std::env::remove_var("AUTOMATION_MASTER_KEY_B64");
    driver.databases().get(&db).await?.delete().await?;
    result
}
