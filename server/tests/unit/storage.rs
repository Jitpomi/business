use super::*;
#[tokio::test]
async fn ambiguous_or_application_errors_never_replay_database_writes() {
    for error in [
        anyhow::anyhow!("STC2 in a transport message is not an abort proof"),
        anyhow::Error::new(typedb_driver::Error::Other("STC2".into())),
        dog_core::DogError::conflict("Native permission or revision changed").into_anyhow(),
    ] {
        assert!(!definitely_aborted(&error));
        let mut calls = 0;
        let mut saved = Some(error);
        let result = retry_aborted(|| {
            calls += 1;
            let error = saved.take().unwrap();
            async move { Err::<(), _>(error) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
}

#[tokio::test]
#[ignore = "Explicit disposable loopback TypeDB transaction-conflict acceptance; no external providers"]
async fn actual_server_abort_retries_only_the_database_transaction() -> anyhow::Result<()> {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let address = std::env::var("BUSINESS_TEST_CONFLICT_ADDR")?;
    let url = reqwest::Url::parse(&address)?;
    anyhow::ensure!(
        url.scheme() == "http" && url.host_str() == Some("127.0.0.1") && url.port().is_some(),
        "Explicit loopback verifier required"
    );
    crate::tls::initialize();
    let driver = Arc::new(
        dog_typedb::TypeDBDriverFactory::connect(&address, "admin", "password", false).await?,
    );
    let database = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    crate::database::install(&driver, &database).await?;
    let state = TypeDBState {
        driver: driver.clone(),
        database: database.clone(),
    };
    let result=async {
        let tx=driver.transaction(&database,TransactionType::Write).await?;
        tx.query("insert $t isa tenant,has tenant_id 'conflict-fixture',has name 'Conflict acceptance',has status 'active',has revision 'initial';$u isa person,has identity_id 'conflict-owner',has name 'Owner',has status 'active';(tenant:$t,member:$u) isa membership,has membership_role 'owner',has start_at 2020-01-01T00:00:00;").await?;tx.commit().await?;
        let query="match $t isa tenant,has tenant_id 'conflict-fixture';update $t has revision 'desired';insert $r isa resource,has resource_id 'one-resource',has name 'Retained once',has status 'active';(tenant:$t,resource:$r) isa resource_ownership;fetch {'id':$r.resource_id};";
        let calls=AtomicUsize::new(0);
        let result=retry_aborted(||async {
            let attempt=calls.fetch_add(1,Ordering::SeqCst);
            let tx=driver.transaction(&database,TransactionType::Write).await?;
            let rows=documents(&tx,query).await?;
            if attempt==0 {
                let rival=driver.transaction(&database,TransactionType::Write).await?;
                rival.query("match $t isa tenant,has tenant_id 'conflict-fixture';update $t has revision 'rival';").await?;rival.commit().await?;
            }
            tx.commit().await?;
            Ok(rows)
        }).await?;
        anyhow::ensure!(calls.load(Ordering::SeqCst)==2&&result.len()==1,"Expected one rejected transaction and one successful fresh transaction");
        let rows=read(&state,"match $r isa resource;fetch {'id':$r.resource_id};").await?;
        anyhow::ensure!(rows==vec![serde_json::json!({"id":"one-resource"})],"Database replay duplicated a committed resource");
        // A fresh transaction must re-evaluate native authorization after a
        // concurrent revocation; an earlier permitted snapshot is not a grant.
        let at=crate::clock::native_utc();
        let guarded=format!("match $t isa tenant,has tenant_id 'conflict-fixture';$u isa person,has identity_id 'conflict-owner';let $allowed=role_at_least($u,$t,'owner',{at});$allowed==true;select $t;update $t has revision 'guarded';insert $r isa resource,has resource_id 'must-not-exist',has name 'Denied after revocation',has status 'active';(tenant:$t,resource:$r) isa resource_ownership;fetch {{'id':$r.resource_id}};");
        let calls=AtomicUsize::new(0);
        let denied=retry_aborted(||async {
            if calls.fetch_add(1,Ordering::SeqCst)>0{return write_once(&state,&guarded).await;}
            let tx=driver.transaction(&database,TransactionType::Write).await?;
            let rows=documents(&tx,&guarded).await?;anyhow::ensure!(rows.len()==1);
            let rival=driver.transaction(&database,TransactionType::Write).await?;
            rival.query("match $t isa tenant,has tenant_id 'conflict-fixture';$u isa person,has identity_id 'conflict-owner';update $t has revision 'revoked';$u has status 'suspended';").await?;rival.commit().await?;
            tx.commit().await?;Ok(rows[0].clone())
        }).await;
        anyhow::ensure!(denied.is_err()&&calls.load(Ordering::SeqCst)==2,"Retry did not re-evaluate native revocation");
        anyhow::ensure!(read(&state,"match $r isa resource,has resource_id 'must-not-exist';fetch {'id':$r.resource_id};").await?.is_empty(),"Revoked owner left an unauthorized resource");
        Ok::<_,anyhow::Error>(())
    }.await;
    if result.is_ok() {
        driver.databases().get(&database).await?.delete().await?;
    } else {
        eprintln!("Retained conflict verifier {database}");
    }
    result
}
