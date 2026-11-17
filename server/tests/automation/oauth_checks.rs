use super::*;
use server::{
    automation::oauth::{Grant, Provider},
    database,
    typedb::TypeDBState,
};
use std::sync::Mutex;
struct OAuthPlugin {
    provider: Arc<MockProvider>,
}
struct MockProvider {
    calls: AtomicUsize,
    verifier: Mutex<String>,
    revoke: Mutex<Option<(Arc<TypeDBState>, String)>>,
}
#[async_trait::async_trait]
impl Provider for MockProvider {
    fn describe(&self) -> Value {
        json!({"configured":true,"permissions":["Test only"],"pkce":true})
    }
    fn authorize(&self, state: &str, verifier: &str) -> Result<String> {
        *self.verifier.lock().unwrap() = verifier.into();
        Ok(format!("https://provider.example/authorize?state={state}"))
    }
    async fn exchange(&self, code: &str, verifier: &str) -> Result<Grant> {
        ensure!(code == "synthetic-code" && verifier == *self.verifier.lock().unwrap());
        self.calls.fetch_add(1, Ordering::SeqCst);
        let revoke = self.revoke.lock().unwrap().take();
        if let Some((state, actor)) = revoke {
            role(&state, &actor, "member").await?;
        }
        Ok(Grant {
            config: json!({"account":"provider-confirmed"}),
            secrets: json!({"token":"synthetic-private-token"}),
        })
    }
}
#[async_trait::async_trait]
impl Connector for OAuthPlugin {
    fn name(&self) -> &str {
        "oauth-test"
    }
    fn oauth(&self) -> Option<Arc<dyn Provider>> {
        Some(self.provider.clone())
    }
    fn actions(&self) -> Vec<ActionSpec> {
        vec![ActionSpec {
            name: "read".into(),
            effect: Effect::Read,
        }]
    }
    fn validate(&self, c: &Value, s: &Value) -> Result<()> {
        ensure!(c["account"] == "provider-confirmed" && s["token"] == "synthetic-private-token");
        Ok(())
    }
    async fn verify(&self, _: &Value, _: &Value) -> Result<()> {
        Ok(())
    }
    async fn execute(&self, _: &Invocation, _: &str, _: Value) -> Result<Value> {
        Ok(json!({"ok":true}))
    }
}
async fn role(state: &TypeDBState, actor: &str, role: &str) -> Result<()> {
    write(state,&format!("match $t isa tenant;$u isa person,has identity_id {}; $m isa membership,links(tenant:$t,member:$u);select $t,$m,$u;update $m has membership_role {};fetch {{'id':$u.identity_id}};",server::automation::store::q(actor),server::automation::store::q(role))).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "Explicit disposable TypeDB; synthetic OAuth provider, no real provider writes"]
async fn oauth_is_native_tenant_actor_expiry_and_replay_fenced() -> Result<()> {
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
    database::install(&driver, &db).await?;
    let state = Arc::new(TypeDBState {
        driver: driver.clone(),
        database: db.clone(),
    });
    let result=async{
  let tenant=uuid::Uuid::new_v4().to_string();let other=uuid::Uuid::new_v4().to_string();let actor=uuid::Uuid::new_v4().to_string();let peer=uuid::Uuid::new_v4().to_string();
  let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
  tx.query(format!(r#"insert $t isa tenant,has tenant_id "{tenant}",has status "active",has name "OAuth",has revision "initial";$b isa tenant,has tenant_id "{other}",has status "active",has name "Other",has revision "initial";$u isa person,has identity_id "{actor}",has name "Owner",has status "active";$p isa person,has identity_id "{peer}",has name "Peer",has status "active";(tenant:$t,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;(tenant:$t,member:$p) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;(tenant:$b,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;"#)).await?;tx.commit().await?;
  let provider=Arc::new(MockProvider{calls:AtomicUsize::new(0),verifier:Mutex::new(String::new()),revoke:Mutex::new(None)});
  let mut catalog=Catalog::default();catalog.register(Arc::new(OAuthPlugin{provider:provider.clone()}))?;
  let e=Engine{store:Store::new(state.clone(),&tenant)?.for_owner(&actor)?,catalog:Arc::new(catalog),vault:Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")?};
  let peer_engine=Engine{store:Store::new(state.clone(),&tenant)?.for_owner(&peer)?,..e.clone()};
  let other_engine=Engine{store:Store::new(state.clone(),&other)?.for_owner(&actor)?,..e.clone()};
  let begun=e.oauth_begin("first","oauth-test").await?;let handoff=begun["state"].as_str().unwrap();
  let records=e.store.list("oauth",100).await?;let serialized=records[0].value.to_string();ensure!(!serialized.contains(handoff)&&!serialized.contains(&*provider.verifier.lock().unwrap())&&!serialized.contains("synthetic-private-token"));
  ensure!(peer_engine.oauth_finish(handoff,Some("synthetic-code"),false).await.is_err());ensure!(other_engine.oauth_finish(handoff,Some("synthetic-code"),false).await.is_err());ensure!(provider.calls.load(Ordering::SeqCst)==0);
  let connected=e.oauth_finish(handoff,Some("synthetic-code"),false).await?;ensure!(connected["verified"]==false&&!connected.to_string().contains("secret"));ensure!(e.oauth_finish(handoff,Some("synthetic-code"),false).await.is_err());ensure!(provider.calls.load(Ordering::SeqCst)==1);
  e.verify_connection("first").await?;ensure!(e.connection("first").await?.2["token"]=="synthetic-private-token");
  let b=e.oauth_begin("denied","oauth-test").await?;let s=b["state"].as_str().unwrap();ensure!(e.oauth_finish(s,None,true).await?["cancelled"]==true);ensure!(e.oauth_finish(s,Some("synthetic-code"),false).await.is_err());ensure!(provider.calls.load(Ordering::SeqCst)==1);
  let b=e.oauth_begin("expired","oauth-test").await?;let s=b["state"].as_str().unwrap();let records=e.store.list("oauth",100).await?;let old=records.iter().find(|r|r.value["id"]=="expired").unwrap();e.store.cas(old,&old.value,"pending",0).await?;ensure!(e.oauth_finish(s,Some("synthetic-code"),false).await.is_err());ensure!(provider.calls.load(Ordering::SeqCst)==1);
  let b=e.oauth_begin("revoked","oauth-test").await?;let s=b["state"].as_str().unwrap();role(&state,&actor,"member").await?;ensure!(e.oauth_finish(s,Some("synthetic-code"),false).await.is_err());ensure!(provider.calls.load(Ordering::SeqCst)==1);role(&state,&actor,"owner").await?;
  *provider.revoke.lock().unwrap()=Some((state.clone(),actor.clone()));ensure!(e.oauth_finish(s,Some("synthetic-code"),false).await.is_err());role(&state,&actor,"owner").await?;ensure!(e.store.get("connection","revoked").await?.is_none());ensure!(e.oauth_finish(s,Some("synthetic-code"),false).await.is_err());ensure!(provider.calls.load(Ordering::SeqCst)==2);
  ensure!(e.connection("first").await?.1.verified);
  e.store.insert("settings","billing",&json!({"sandbox":true,"default_financial":"first","notification":null,"reminder_days":7,"allowed_test_recipients":[]}),"configured",0).await?;
  e.disconnect("first").await?;
  let managed=server::automation::managed::Managed{state:state.clone(),catalog:e.catalog.clone(),vault:e.vault.clone()};
  use server::finance::RuntimeSource;
  ensure!(managed.resolve(&tenant).await.is_err(),"Disabled financial connection must fail closed");
  let metadata=managed.configuration(&tenant).await?;
  ensure!(metadata.sandbox&&metadata.default_financial.as_deref()==Some("first"),"Broken plugin must not block account context or connection repair");
  let b=e.oauth_begin("concurrent","oauth-test").await?;let s=b["state"].as_str().unwrap();
  let (left,right)=tokio::join!(e.oauth_finish(s,Some("synthetic-code"),false),e.oauth_finish(s,Some("synthetic-code"),false));
  ensure!(left.is_ok()!=right.is_ok(),"Concurrent callbacks both won or both failed");ensure!(provider.calls.load(Ordering::SeqCst)==3,"Authorization code exchanged more than once");
  Ok::<_,anyhow::Error>(())
 }.await;
    driver.databases().get(&db).await?.delete().await?;
    result
}

async fn write(state: &TypeDBState, query: &str) -> Result<()> {
    let tx = state
        .driver
        .transaction(&state.database, typedb_driver::TransactionType::Write)
        .await?;
    let rows = database::documents(&tx, query).await?;
    ensure!(!rows.is_empty());
    tx.commit().await?;
    Ok(())
}
