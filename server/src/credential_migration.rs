//! Offline, repeatable separation of people from local authentication credentials.
//! Call only after stopping writers and verifying an operator-owned backup.
use crate::database::{documents, target, SCHEMA, SCHEMA_POLICY};
use anyhow::{ensure, Context, Result};
use typedb_driver::{TransactionType, TypeDBDriver};

pub async fn migrate(driver: &TypeDBDriver, name: &str) -> Result<usize> {
    target(name)?;
    let schema = driver.databases().get(name).await?.schema().await?;
    ensure!(
        schema.contains("entity person") && schema.contains("scope_identity"),
        "Canonical identity model required"
    );
    let person = schema
        .split("entity person")
        .nth(1)
        .context("Missing person schema")?
        .split(';')
        .next()
        .unwrap();
    let legacy = person.contains("owns password_hash");
    if !legacy {
        crate::database::verify(driver, name).await?;
        return Ok(0);
    }
    let tx = driver.transaction(name, TransactionType::Schema).await?;
    if !schema.contains("relation authentication_credential") {
        let start = SCHEMA.find("attribute credential_id").unwrap();
        let end = SCHEMA[start..].find("entity service_account").unwrap() + start;
        tx.query(format!("define {}", &SCHEMA[start..end])).await?;
    }
    // Relax only the old owners, so values can move without deleting attributes.
    tx.query("redefine person owns password_hash @card(0..1);")
        .await?;
    tx.query("redefine person owns credential_version @card(0..1);")
        .await?;
    tx.commit().await?;
    let tx = driver.transaction(name, TransactionType::Write).await?;
    let rows=documents(&tx,"match $p isa person,has password_hash $hash,has credential_version $version;fetch {'id':$p.identity_id,'email':$p.email,'hash':$hash,'version':$version};").await?;
    let q = |s: &str| serde_json::to_string(s).unwrap();
    for row in &rows {
        let get = |k: &str| row[k].as_str().context("Invalid legacy credential");
        let id = get("id")?;
        let found=documents(&tx,&format!("match $p isa person,has identity_id {}; $c isa authentication_credential,links(person:$p);fetch {{'id':$c.credential_id}};",q(id))).await?;
        ensure!(
            found.is_empty(),
            "Person already has credentials; refusing to overwrite or merge"
        );
        tx.query(format!("match $p isa person,has identity_id {},has password_hash $hash,has credential_version $version;insert (person:$p) isa password_credential,has credential_id {},has login_email {},has $hash,has $version;delete has $hash of $p;has $version of $p;",q(id),q(id),q(get("email")?))).await?;
        let copied=documents(&tx,&format!("match $p isa person,has identity_id {}; $c isa password_credential,links(person:$p);fetch {{'hash':$c.password_hash,'version':$c.credential_version,'email':$c.login_email}};",q(id))).await?;
        ensure!(
            copied.len() == 1
                && copied[0]["hash"] == row["hash"]
                && copied[0]["version"] == row["version"]
                && copied[0]["email"] == row["email"],
            "Credential preservation check failed"
        );
    }
    // Any incomplete legacy ownership means an interrupted/incompatible migration.
    let remaining=documents(&tx,"match $p isa person; {$p has password_hash $v;} or {$p has credential_version $v;};fetch {'id':$p.identity_id};").await?;
    ensure!(
        remaining.is_empty(),
        "Unpaired legacy credentials; nothing committed"
    );
    tx.commit().await?;
    let tx = driver.transaction(name, TransactionType::Schema).await?;
    tx.query("undefine owns password_hash from person;owns credential_version from person;")
        .await?;
    if person.contains("@key") {
        tx.query("undefine @key from person owns email;").await?;
        tx.query("define person owns email @unique @card(0..1);")
            .await?;
    }
    let policy = SCHEMA_POLICY.split("# Credential records").nth(1).unwrap();
    tx.query(format!("define {}", policy.split_once('\n').unwrap().1))
        .await?;
    tx.commit().await?;
    crate::database::verify(driver, name).await?;
    Ok(rows.len())
}
