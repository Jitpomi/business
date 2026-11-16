//! Migration refuses unproven authority and unknown schema without losing data.
use super::*;
use server::database::{self, snapshot};
use std::path::Path;
use typedb_driver::TransactionType;
async fn backup(driver: &typedb_driver::TypeDBDriver, name: &str, dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    driver
        .databases()
        .get(name)
        .await?
        .export_to_file(
            dir.join(format!("{name}.tql")),
            dir.join(format!("{name}.typedb")),
        )
        .await?;
    std::fs::write(
        dir.join(format!("{name}.records.json")),
        serde_json::to_vec_pretty(&snapshot::records(driver, name).await?)?,
    )?;
    Ok(())
}
#[tokio::test]
#[ignore = "Development TypeDB only; preserved synthetic migration fixtures, no provider/mail calls"]
async fn consent_shadow_preserves_graph_and_refuses_unproven_or_custom_work() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(
        std::env::var("TYPEDB_DB")? == "business_dev",
        "Development operator required"
    );
    let root = std::path::PathBuf::from(std::env::var("CONSENT_TEST_ROOT")?);
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        root.is_absolute() && root.metadata()?.permissions().mode() & 0o077 == 0,
        "Private absolute evidence directory required"
    );
    let driver = dog_typedb::TypeDBDriverFactory::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let source = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    database::install(&driver, &source).await?;
    let tx = driver.transaction(&source, TransactionType::Write).await?;
    tx.query("insert $t isa tenant,has tenant_id 'consent-migration-merchant',has name 'Synthetic merchant',has status 'active',has revision 'initial';$b isa tenant,has tenant_id 'consent-migration-buyer',has name 'Synthetic buyer',has status 'active',has revision 'initial';$c isa bill_customer,has bill_key 'migration-customer',has name 'Preserved useful work',has bill_email 'test@example.invalid',has bill_policy 'customer',has bill_choice 'manual',has bill_provider 'custom-bank',has bill_bindings '{}',has bill_method_plugin 'custom-bank',has bill_payment_method 'bank/test-method',has bill_consent '',has bill_revision 'terms-v1';(merchant:$t,buyer:$b,customer:$c) isa bill_account;").await?;
    tx.commit().await?;
    let before = snapshot::graph(&driver, &source, &[]).await?;
    let first = root.join("valid");
    backup(&driver, &source, &first).await?;
    let good = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    database::verify_runtime(&driver, &source).await?;
    server::consent_migration::import(&driver, &source, &good, &first).await?;
    ensure!(
        snapshot::graph(&driver, &good, &[]).await? == before,
        "Successful migration lost work"
    );
    let tx = driver.transaction(&source, TransactionType::Write).await?;
    tx.query("match $c isa bill_customer;update $c has bill_choice 'automatic',has bill_consent 'terms-v1';").await?;
    tx.commit().await?;
    let active = snapshot::graph(&driver, &source, &[]).await?;
    let second = root.join("unproven");
    backup(&driver, &source, &second).await?;
    let rejected = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    let error = server::consent_migration::import(&driver, &source, &rejected, &second)
        .await
        .expect_err("Unproven legacy consent became authority");
    ensure!(
        error.to_string().contains("Legacy active consent"),
        "Wrong refusal: {error}"
    );
    ensure!(
        snapshot::graph(&driver, &source, &[]).await? == active
            && snapshot::graph(&driver, &rejected, &[]).await? == active,
        "Refused cutover lost records"
    );
    let tx = driver.transaction(&source, TransactionType::Schema).await?;
    tx.query("define fun useful_custom_work() -> boolean:match let $ok=true;return first $ok;")
        .await?;
    tx.commit().await?;
    let third = root.join("custom");
    backup(&driver, &source, &third).await?;
    let custom = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    let error = server::consent_migration::import(&driver, &source, &custom, &third)
        .await
        .expect_err("Unknown useful schema silently retired");
    ensure!(
        error.to_string().contains("Custom schema"),
        "Wrong custom-schema refusal: {error}"
    );
    ensure!(
        !driver.databases().contains(&custom).await?
            && snapshot::graph(&driver, &source, &[]).await? == active,
        "Unknown-schema refusal changed useful work"
    );
    std::fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":true,"source":source,"successful_shadow":good,"rejected_shadow_retained":rejected,"graph_and_roles_preserved":true,"unproven_authority_refused":true,"unknown_schema_refused":true}),
        )?,
    )?;
    Ok(())
}
