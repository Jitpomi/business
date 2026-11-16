//! Offline credential evacuation. Only non-secret issuer/subject links remain in TypeDB.
use crate::database::{documents, target, SCHEMA, SCHEMA_POLICY};
use anyhow::{ensure, Context, Result};
use serde_json::json;
use typedb_driver::{TransactionType, TypeDBDriver};
pub async fn migrate(driver: &TypeDBDriver, name: &str) -> Result<usize> {
    target(name)?;
    let schema = driver.databases().get(name).await?.schema().await?;
    ensure!(
        schema.contains("entity person") && schema.contains("scope_identity"),
        "Canonical identity model required"
    );
    let _ = actor_policy_updates(&schema)?;
    if !schema.contains("attribute password_hash") {
        evacuate_plugin_secrets(driver, name).await?;
        crate::database::verify(driver, name).await?;
        return Ok(0);
    }
    let store = crate::auth::plugins::local::Local::configured(name)?;
    let person = schema
        .split("entity person")
        .nth(1)
        .context("Missing person schema")?
        .split(';')
        .next()
        .unwrap();
    let legacy = person.contains("owns password_hash");
    let read = driver.transaction(name, TransactionType::Read).await?;
    let query = if legacy {
        "match $p isa person,has password_hash $hash,has credential_version $version;fetch {'id':$p.identity_id,'subject':$p.identity_id,'email':$p.email,'hash':$hash,'version':$version};"
    } else {
        "match $p isa person;$c isa password_credential,links(person:$p);fetch {'id':$p.identity_id,'subject':$c.credential_id,'email':$c.login_email,'hash':$c.password_hash,'version':$c.credential_version};"
    };
    let accounts = documents(&read, query).await?;
    let links = if schema.contains("entity account_link") {
        documents(&read,"match $l isa account_link;fetch {'key':$l.token_key,'kind':$l.link_kind,'email':$l.email,'target':$l.link_target,'version':$l.link_version,'expires':$l.link_expires,'used':$l.link_used};").await?
    } else {
        vec![]
    };
    let revoked = if schema.contains("entity auth_revocation") {
        documents(
            &read,
            "match $r isa auth_revocation;fetch {'key':$r.token_key};",
        )
        .await?
    } else {
        vec![]
    };
    read.close().await?;
    let field = |v: &serde_json::Value, k: &str| -> Result<String> {
        Ok(v[k]
            .as_str()
            .context("Invalid migration record")?
            .to_owned())
    };
    // Durable external copy precedes any graph deletion. Imports refuse conflicts.
    for account in &accounts {
        store.import(
            &field(account, "subject")?,
            &field(account, "email")?,
            &field(account, "hash")?,
            &field(account, "version")?,
        )?;
    }
    for link in &links {
        let key = field(link, "key")?;
        let invitation =
            uuid::Uuid::parse_str(&crate::auth::binding_key("invitation", &key)[..32])?.to_string();
        store.import_challenge(&key,&json!({"kind":link["kind"],"email":link["email"],"target":link["target"],"version":link["version"],"expires":link["expires"],"invitation":invitation}),link["used"]==true)?;
    }
    for record in &revoked {
        store.revoke(&field(record, "key")?, i64::MAX)?;
    }
    evacuate_plugin_secrets(driver, name).await?;
    let tx = driver.transaction(name, TransactionType::Schema).await?;
    if !schema.contains("relation external_identity") {
        let start = SCHEMA.find("attribute auth_binding_key").unwrap();
        let end = SCHEMA[start..].find("entity service_account").unwrap() + start;
        tx.query(format!("define {}", &SCHEMA[start..end])).await?;
    }
    if !schema.contains("entity account_invitation") {
        tx.query(crate::database::ACCOUNT_LINKS).await?;
    }
    if legacy {
        tx.query("redefine person owns password_hash @card(0..1);")
            .await?;
        tx.query("redefine person owns credential_version @card(0..1);")
            .await?;
    }
    tx.commit().await?;
    let tx = driver.transaction(name, TransactionType::Write).await?;
    let q = |v: &str| serde_json::to_string(v).unwrap();
    for account in &accounts {
        let id = field(account, "id")?;
        let subject = field(account, "subject")?;
        let key = crate::auth::binding_key("urn:jitpomi:local", &subject);
        let old=documents(&tx,&format!("match $l isa external_identity,has auth_binding_key {};fetch {{'subject':$l.auth_subject}};",q(&key))).await?;
        if old.is_empty() {
            tx.query(format!("match $p isa person,has identity_id {};insert (identity:$p) isa external_identity,has auth_binding_key {},has auth_issuer 'urn:jitpomi:local',has auth_subject {};",q(&id),q(&key),q(&subject))).await?;
        }
    }
    for link in links.iter().filter(|v| v["kind"] == "invite") {
        let invitation = uuid::Uuid::parse_str(
            &crate::auth::binding_key("invitation", &field(link, "key")?)[..32],
        )?
        .to_string();
        let old=documents(&tx,&format!("match $l isa account_invitation,has invitation_id {};fetch {{'id':$l.invitation_id}};",q(&invitation))).await?;
        if old.is_empty() {
            tx.query(format!("insert $l isa account_invitation,has invitation_id {},has invitation_email {},has invitation_target {},has invitation_revision {},has invitation_expires {},has invitation_used {};",q(&invitation),q(&field(link,"email")?),q(&field(link,"target")?),q(&field(link,"version")?),link["expires"],link["used"])).await?;
        }
    }
    if legacy {
        tx.query("match $p isa person,has password_hash $hash,has credential_version $version;delete has $hash of $p;has $version of $p;").await?;
    } else {
        tx.query("match $c isa authentication_credential;delete $c;")
            .await?;
    }
    if schema.contains("entity account_link") {
        tx.query("match $l isa account_link;delete $l;").await?;
    }
    if schema.contains("entity auth_revocation") {
        tx.query("match $r isa auth_revocation;delete $r;").await?;
    }
    tx.commit().await?;
    let tx = driver.transaction(name, TransactionType::Schema).await?;
    if schema.contains("fun scope_password_credential") {
        tx.query("undefine fun scope_password_credential;").await?;
    }
    if legacy {
        tx.query("undefine owns password_hash from person;owns credential_version from person;")
            .await?;
    } else {
        tx.query("undefine plays authentication_credential:person from person;")
            .await?;
        tx.query("undefine password_credential;").await?;
        tx.query("undefine authentication_credential;").await?;
        tx.query("undefine credential_id;login_email;").await?;
    }
    if person.contains("@key") {
        tx.query("undefine @key from person owns email;").await?;
        tx.query("define person owns email @unique @card(0..1);")
            .await?;
    }
    if schema.contains("entity account_link") {
        tx.query("undefine account_link;").await?;
        tx.query("undefine link_kind;link_expires;link_used;link_target;link_version;")
            .await?;
    }
    if schema.contains("entity auth_revocation") {
        tx.query("undefine auth_revocation;").await?;
    }
    if schema.contains("attribute token_key") {
        tx.query("undefine token_key;").await?;
    }
    tx.query("undefine password_hash;credential_version;")
        .await?;
    for source in actor_policy_updates(&schema)? {
        tx.query(format!("redefine {source}")).await?;
    }
    let policies = SCHEMA_POLICY.split("# Identity links").nth(1).unwrap();
    tx.query(format!("define {}", policies.split_once('\n').unwrap().1))
        .await?;
    tx.commit().await?;
    crate::database::verify(driver, name).await?;
    Ok(accounts.len())
}

/// Move legacy encrypted plugin credentials out of graph payloads. Writers must be stopped.
pub async fn evacuate_plugin_secrets(driver: &TypeDBDriver, name: &str) -> Result<usize> {
    target(name)?;
    let schema = driver.databases().get(name).await?.schema().await?;
    if !schema.contains("entity automation_record") {
        return Ok(0);
    }
    let tx = driver.transaction(name, TransactionType::Read).await?;
    let rows=documents(&tx,"match $r isa automation_record,has auto_kind 'connection';fetch {'key':$r.auto_key,'tenant':$r.auto_tenant,'payload':$r.auto_payload};").await?;
    tx.close().await?;
    let mut candidates = Vec::new();
    for row in rows {
        let payload: serde_json::Value = serde_json::from_str(
            row["payload"]
                .as_str()
                .context("Invalid connection record")?,
        )?;
        if payload.get("sealed").is_some() {
            candidates.push((row, payload));
        }
    }
    if candidates.is_empty() {
        return Ok(0);
    }
    let vault = crate::automation::vault::Vault::from_env()?;
    let tx = driver.transaction(name, TransactionType::Write).await?;
    let q = |s: &str| serde_json::to_string(s).unwrap();
    for (row, payload) in &mut candidates {
        let context = format!(
            "{}/{}/{}",
            row["tenant"].as_str().context("Missing tenant")?,
            payload["id"].as_str().context("Missing connection ID")?,
            payload["revision"]
                .as_str()
                .context("Missing connection revision")?
        );
        let old = payload["sealed"]
            .as_str()
            .context("Invalid legacy credential field")?;
        let reference = if old.is_empty() {
            String::new()
        } else if old.starts_with("secret:") {
            let _ = vault.open(&context, old).await?;
            old.to_owned()
        } else {
            vault.import_inline(&context, old).await?
        };
        if !reference.is_empty() {
            let _ = vault.open(&context, &reference).await?;
        }
        payload.as_object_mut().unwrap().remove("sealed");
        payload["secret_ref"] = json!(reference);
        let changed=documents(&tx,&format!("match $r isa automation_record,has auto_key {},has auto_payload {};update $r has auto_payload {},has auto_revision {};fetch {{'changed':true}};",q(row["key"].as_str().unwrap()),q(row["payload"].as_str().unwrap()),q(&payload.to_string()),q(&uuid::Uuid::new_v4().to_string()))).await?;
        ensure!(
            changed.len() == 1,
            "Connection changed during migration; nothing committed"
        );
    }
    tx.commit().await?;
    Ok(candidates.len())
}

fn actor_policy_updates(schema: &str) -> Result<Vec<String>> {
    let mut updates = Vec::new();
    let definitions = regex::Regex::new(r"(?ms)^fun (\w+)\(.*?return (?:check|first [^;]+);")?;
    let compact = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    for definition in definitions.captures_iter(SCHEMA_POLICY) {
        let source = definition.get(0).unwrap().as_str();
        if !source.contains("$i isa identity, has identity_id == $actor;") {
            continue;
        }
        let pattern = regex::Regex::new(&format!(
            r"(?s)fun {}\(.*?return check;",
            regex::escape(&definition[1])
        ))?;
        let current = pattern
            .find(schema)
            .context("Missing canonical data policy")?
            .as_str();
        let old = source.replace(
            "$i isa identity, has identity_id == $actor;",
            "$i isa person, has identity_id == $actor;",
        );
        ensure!(
            compact(current) == compact(source) || compact(current) == compact(&old),
            "Custom data policy requires explicit migration review"
        );
        if compact(current) != compact(source) {
            updates.push(source.to_owned());
        }
    }
    Ok(updates)
}
