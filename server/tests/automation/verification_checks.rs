use super::*;
use server::automation::verification::{self, Failure};

#[test]
fn verification_errors_are_actionable_without_private_provider_details() {
    for (status, expected) in [
        (401, 409),
        (403, 409),
        (422, 409),
        (429, 503),
        (500, 502),
        (302, 502),
    ] {
        let error =
            verification::response(reqwest::StatusCode::from_u16(status).unwrap()).unwrap_err();
        assert_eq!(
            dog_core::DogError::normalize(verification::public(error)).code(),
            expected
        );
    }
    assert!(verification::response(reqwest::StatusCode::OK).is_ok());
    let error = dog_core::DogError::normalize(verification::public(anyhow::anyhow!(
        "private-token https://private.example/secret?key=private-token"
    )));
    assert_eq!(error.code(), 502);
    assert!(!error.to_json().to_string().contains("private-token"));
    assert_eq!(
        dog_core::DogError::normalize(verification::public(Failure::Unavailable.into())).code(),
        503
    );
}
struct Probe {
    mode: AtomicUsize,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl Connector for Probe {
    fn name(&self) -> &str {
        "verification-test"
    }
    fn actions(&self) -> Vec<ActionSpec> {
        vec![]
    }
    fn validate(&self, _: &Value, _: &Value) -> Result<()> {
        Ok(())
    }
    async fn verify(&self, _: &Value, _: &Value) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.mode.load(Ordering::SeqCst) {
            0 => Err(Failure::Rejected(401).into()),
            1 => anyhow::bail!("private-provider-token"),
            _ => Ok(()),
        }
    }
    async fn execute(&self, _: &Invocation, _: &str, _: Value) -> Result<Value> {
        anyhow::bail!("No execution capability")
    }
}
#[tokio::test]
#[ignore = "Explicit disposable TypeDB; verification failures and native tenant isolation; no provider calls"]
async fn failed_verification_preserves_connection_and_native_tenant_boundaries() -> Result<()> {
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
        let state=Arc::new(server::typedb::TypeDBState{driver:driver.clone(),database:db.clone()});
        let tenant=uuid::Uuid::new_v4().to_string();let other=uuid::Uuid::new_v4().to_string();let owner=uuid::Uuid::new_v4().to_string();
        let tx=driver.transaction(&db,typedb_driver::TransactionType::Write).await?;
        tx.query(format!(r#"insert $t isa tenant,has tenant_id "{tenant}",has name "Verification fixture",has status "active",has revision "initial";$b isa tenant,has tenant_id "{other}",has name "Other fixture",has status "active",has revision "initial";$u isa person,has identity_id "{owner}",has name "Test owner",has status "active";(tenant:$t,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;"#)).await?;tx.commit().await?;
        let probe=Arc::new(Probe{mode:AtomicUsize::new(0),calls:AtomicUsize::new(0)});let mut catalog=Catalog::default();catalog.register(probe.clone())?;
        let e=Engine{store:Store::new(state.clone(),&tenant)?.for_owner(&owner)?,catalog:Arc::new(catalog),vault:Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")?};
        e.connect("account","verification-test",json!({}),Value::Null).await?;
        let initial=e.store.get("connection","account").await?.unwrap();
        for (mode,expected) in [(0,409),(1,502)] {
            probe.mode.store(mode,Ordering::SeqCst);
            let error=dog_core::DogError::normalize(e.verify_connection("account").await.unwrap_err());ensure!(error.code()==expected&&!error.to_json().to_string().contains("private-provider-token"));
            let after=e.store.get("connection","account").await?.unwrap();ensure!(initial.revision==after.revision&&initial.value==after.value,"Failed verification changed stored connection");
        }
        let foreign=Engine{store:Store::new(state,&other)?.for_owner(&owner)?,..e.clone()};
        ensure!(foreign.verify_connection("account").await.is_err());ensure!(probe.calls.load(Ordering::SeqCst)==2,"Foreign tenant reached provider verification");
        probe.mode.store(2,Ordering::SeqCst);ensure!(e.verify_connection("account").await?["verified"]==true);
        e.disconnect("account").await?;let error=dog_core::DogError::normalize(e.verify_connection("account").await.unwrap_err());ensure!(error.code()==409);ensure!(probe.calls.load(Ordering::SeqCst)==3,"Disabled connection reached provider");
        Ok::<_,anyhow::Error>(())
    }.await;
    driver.databases().get(&db).await?.delete().await?;
    result
}
