//! Shared read-only database preservation evidence.
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use typedb_driver::{TransactionType, TypeDBDriver};
pub async fn inventory(driver: &TypeDBDriver, name: &str) -> Result<Value> {
    let schema = driver.databases().get(name).await?.schema().await?;
    let tx = driver.transaction(name, TransactionType::Read).await?;
    let mut counts = BTreeMap::new();
    for line in schema.lines() {
        if let Some(label) = line
            .trim()
            .strip_prefix("entity ")
            .or_else(|| line.trim().strip_prefix("relation "))
        {
            let label = label.split([',', ' ', ';']).next().unwrap();
            let rows = crate::database::documents(
                &tx,
                &format!("match $x isa {label};reduce $n=count;fetch {{'count':$n}};"),
            )
            .await?;
            counts.insert(label.to_owned(), rows[0]["count"].clone());
        }
    }
    tx.close().await?;
    Ok(
        json!({"database":name,"canonical":schema.contains("scope_bill_customer"),"plugin_schema":schema.contains("attribute bill_bindings"),"counts":counts}),
    )
}
pub async fn records(driver: &TypeDBDriver, name: &str) -> Result<BTreeMap<String, Value>> {
    let schema = driver.databases().get(name).await?.schema().await?;
    let tx = driver.transaction(name, TransactionType::Read).await?;
    let mut records = BTreeMap::new();
    for line in schema.lines() {
        if let Some(label) = line
            .trim()
            .strip_prefix("entity ")
            .or_else(|| line.trim().strip_prefix("relation "))
        {
            let label = label.split([',', ' ', ';']).next().unwrap();
            for row in crate::database::documents(
                &tx,
                &format!("match $x isa {label};fetch {{'iid':iid($x),'data':{{$x.*}}}};"),
            )
            .await?
            {
                records.insert(
                    row["iid"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing internal ID"))?
                        .to_owned(),
                    row["data"].clone(),
                );
            }
        }
    }
    tx.close().await?;
    Ok(records)
}

/// Portable proof includes relation role bindings. TypeDB import remaps internal
/// IDs, so compare full logical graphs rather than treating an IID as a user ID.
pub async fn graph(
    driver: &TypeDBDriver,
    name: &str,
    ignore: &[&str],
) -> Result<BTreeMap<String, usize>> {
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeSet, HashMap, HashSet};
    use typeql::{
        query::{schema::SchemaQuery, QueryStructure},
        schema::definable::{type_::CapabilityBase, Definable},
    };
    #[derive(Default)]
    struct Type {
        kind: String,
        abstract_: bool,
        parent: Option<String>,
        roles: BTreeSet<String>,
    }
    #[derive(Default)]
    struct Node {
        type_: String,
        attributes: Value,
        roles: BTreeMap<String, Vec<String>>,
    }
    let schema = driver.databases().get(name).await?.schema().await?;
    let mut types: BTreeMap<String, Type> = BTreeMap::new();
    for query in typeql::parse_queries(&schema)? {
        let QueryStructure::Schema(SchemaQuery::Define(define)) = query.structure else {
            anyhow::bail!("Unexpected schema export")
        };
        for definition in define.definables {
            if let Definable::TypeDeclaration(t) = definition {
                let entry = types.entry(t.label.to_string()).or_default();
                if let Some(kind) = t.kind {
                    entry.kind = kind.to_string();
                }
                entry.abstract_ |= t
                    .annotations
                    .iter()
                    .any(|a| matches!(a, typeql::Annotation::Abstract(_)));
                for capability in t.capabilities {
                    match capability.base {
                        CapabilityBase::Sub(s) => {
                            entry.parent = Some(s.supertype_label.to_string())
                        }
                        CapabilityBase::Relates(r) => {
                            entry.roles.insert(r.related.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let tx = driver.transaction(name, TransactionType::Read).await?;
    let mut nodes: HashMap<String, Node> = HashMap::new();
    for (label, t) in &types {
        if t.abstract_ || !matches!(t.kind.as_str(), "entity" | "relation") {
            continue;
        }
        for row in crate::database::documents(
            &tx,
            &format!("match $x isa! {label};fetch {{'iid':iid($x),'attributes':{{$x.*}}}};"),
        )
        .await?
        {
            let mut attributes = row["attributes"].clone();
            for attribute in ignore {
                attributes
                    .as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("Invalid attributes"))?
                    .remove(*attribute);
            }
            nodes.insert(
                row["iid"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing IID"))?
                    .into(),
                Node {
                    type_: label.clone(),
                    attributes,
                    roles: BTreeMap::new(),
                },
            );
        }
    }
    for (label, t) in &types {
        if t.abstract_ || t.kind != "relation" {
            continue;
        }
        let mut roles = t.roles.clone();
        let mut parent = t.parent.as_deref();
        let mut visited = HashSet::new();
        while let Some(p) = parent {
            anyhow::ensure!(visited.insert(p), "Cyclic type hierarchy");
            let Some(ancestor) = types.get(p) else {
                break;
            };
            roles.extend(ancestor.roles.iter().cloned());
            parent = ancestor.parent.as_deref();
        }
        for role in roles {
            let role = role.rsplit(':').next().unwrap();
            for row in crate::database::documents(&tx,&format!("match $r isa! {label},links({role}:$p);fetch {{'relation':iid($r),'player':iid($p)}};")).await?{
    nodes.get_mut(row["relation"].as_str().unwrap()).ok_or_else(||anyhow::anyhow!("Missing relation"))?.roles.entry(role.into()).or_default().push(row["player"].as_str().unwrap().into());
   }
        }
    }
    tx.close().await?;
    fn digest(
        id: &str,
        nodes: &HashMap<String, Node>,
        cache: &mut HashMap<String, String>,
        visiting: &mut HashSet<String>,
    ) -> Result<String> {
        if let Some(value) = cache.get(id) {
            return Ok(value.clone());
        }
        anyhow::ensure!(
            visiting.insert(id.into()),
            "Cyclic relation graph requires preservation review"
        );
        let node = nodes
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Missing role player"))?;
        let mut roles = BTreeMap::new();
        for (role, players) in &node.roles {
            let mut keys = Vec::new();
            for player in players {
                keys.push(digest(player, nodes, cache, visiting)?);
            }
            keys.sort();
            roles.insert(role, keys);
        }
        let value = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(
                &json!({"type":node.type_,"attributes":node.attributes,"roles":roles})
            )?)
        );
        visiting.remove(id);
        cache.insert(id.into(), value.clone());
        Ok(value)
    }
    let mut result = BTreeMap::new();
    let mut cache = HashMap::new();
    for id in nodes.keys() {
        *result
            .entry(digest(id, &nodes, &mut cache, &mut HashSet::new())?)
            .or_insert(0) += 1;
    }
    Ok(result)
}

/// Quiesced checkpoint shared by database backup and full recovery tooling.
/// Refuses overwrite and proves attributes and relationship roles did not change during export.
pub async fn checkpoint(
    driver: &TypeDBDriver,
    name: &str,
    directory: &std::path::Path,
) -> anyhow::Result<()> {
    use anyhow::ensure;
    let manifest = directory.join(format!("{name}.inventory.json"));
    ensure!(!manifest.exists(), "Backup manifest already exists");
    let before = inventory(driver, name).await?;
    let original_records = records(driver, name).await?;
    let original_graph = graph(driver, name, &[]).await?;
    driver
        .databases()
        .get(name)
        .await?
        .export_to_file(
            directory.join(format!("{name}.tql")),
            directory.join(format!("{name}.typedb")),
        )
        .await?;
    ensure!(records(driver,name).await?==original_records && graph(driver,name,&[]).await?==original_graph && inventory(driver,name).await?==before,"Source changed during export; preserve this attempt and quiesce writers for a new checkpoint");
    std::fs::write(
        directory.join(format!("{name}.records.json")),
        serde_json::to_vec_pretty(&original_records)?,
    )?;
    std::fs::write(
        directory.join(format!("{name}.graph.json")),
        serde_json::to_vec_pretty(&original_graph)?,
    )?;
    std::fs::write(manifest, serde_json::to_vec_pretty(&before)?)?;
    Ok(())
}

/// Online exports are transactional at the server. Build manifests from an isolated import of
/// that exact export, never from separately timed source reads while writers are active.
pub async fn online_checkpoint(
    driver: &TypeDBDriver,
    name: &str,
    directory: &std::path::Path,
) -> anyhow::Result<()> {
    for suffix in [
        "tql",
        "typedb",
        "records.json",
        "graph.json",
        "inventory.json",
    ] {
        anyhow::ensure!(
            !directory.join(format!("{name}.{suffix}")).exists(),
            "Checkpoint destination contains an existing attempt"
        );
    }
    let schema = directory.join(format!("{name}.tql"));
    let data = directory.join(format!("{name}.typedb"));
    driver
        .databases()
        .get(name)
        .await?
        .export_to_file(&schema, &data)
        .await?;
    let reference = format!("business_backend_test_{}", uuid::Uuid::new_v4());
    anyhow::ensure!(
        !driver.databases().contains(&reference).await?,
        "Reference target exists"
    );
    driver
        .databases()
        .import_from_file(&reference, std::fs::read_to_string(schema)?, data)
        .await?;
    crate::database::verify_runtime(driver, &reference).await?;
    for (suffix, value) in [
        (
            "records.json",
            serde_json::to_value(records(driver, &reference).await?)?,
        ),
        (
            "graph.json",
            serde_json::to_value(graph(driver, &reference, &[]).await?)?,
        ),
        ("inventory.json", inventory(driver, &reference).await?),
    ] {
        std::fs::write(
            directory.join(format!("{name}.{suffix}")),
            serde_json::to_vec_pretty(&value)?,
        )?;
    }
    driver.databases().get(&reference).await?.delete().await?;
    Ok(())
}
