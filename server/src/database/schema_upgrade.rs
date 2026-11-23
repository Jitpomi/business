//! Additive schema updates and function replacements in one TypeDB schema transaction.
//! Database identifiers do not determine the application's schema version.
use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use typeql::{
    query::{schema::SchemaQuery, QueryStructure},
    schema::definable::{type_::Type, Definable},
};

#[derive(Debug, serde::Serialize)]
pub struct Plan {
    pub source_sha256: String,
    pub target_sha256: String,
    pub new_types: usize,
    pub added_capabilities: usize,
    pub new_functions: usize,
    pub replaced_functions: Vec<String>,
    pub define: String,
    pub redefine: Vec<String>,
}

fn definitions(schema: &str) -> Result<BTreeMap<String, Definable>> {
    let mut out = BTreeMap::new();
    for query in typeql::parse_queries(schema)? {
        let QueryStructure::Schema(SchemaQuery::Define(define)) = query.structure else {
            anyhow::bail!("Canonical define schema required");
        };
        for def in define.definables {
            let key = match &def {
                Definable::TypeDeclaration(t) => format!("type:{}", t.label),
                Definable::Function(f) => format!("function:{}", f.signature.ident),
                _ => anyhow::bail!("Review unsupported schema definitions before upgrade"),
            };
            ensure!(
                out.insert(key, def).is_none(),
                "Canonical schema must consolidate each definition"
            );
        }
    }
    Ok(out)
}
fn annotations(t: &Type) -> BTreeSet<String> {
    t.annotations.iter().map(ToString::to_string).collect()
}
fn capabilities(t: &Type) -> BTreeSet<String> {
    t.capabilities.iter().map(ToString::to_string).collect()
}
pub fn fingerprint(schema: &str) -> Result<String> {
    let normalized = definitions(schema)?
        .into_iter()
        .map(|(key, definition)| (key, definition.to_string()))
        .collect::<BTreeMap<_, _>>();
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&normalized)?)
    ))
}

/// Retain every existing type, capability, annotation and public function signature.
/// Changed function bodies must come from the independently rehearsed target schema.
pub fn plan(source: &str, target: &str) -> Result<Plan> {
    let old = definitions(source)?;
    let new = definitions(target)?;
    ensure!(
        old.keys().all(|key| new.contains_key(key)),
        "Upgrade would remove existing definitions; nothing changed"
    );
    let mut additions = Vec::new();
    let mut redefine = Vec::new();
    let mut replaced_functions = Vec::new();
    let (mut new_types, mut added_capabilities, mut new_functions) = (0, 0, 0);
    for (key, def) in new {
        match (old.get(&key), def) {
            (None, Definable::TypeDeclaration(t)) => {
                new_types += 1;
                additions.push(t.to_string());
            }
            (None, Definable::Function(f)) => {
                new_functions += 1;
                additions.push(f.to_string());
            }
            (Some(Definable::TypeDeclaration(before)), Definable::TypeDeclaration(mut after)) => {
                ensure!(
                    before.kind == after.kind && annotations(before) == annotations(&after),
                    "Existing type kind or annotations changed: {key}"
                );
                let original = capabilities(before);
                ensure!(
                    original.is_subset(&capabilities(&after)),
                    "Existing capability would change or disappear: {key}"
                );
                after.kind = None;
                after.annotations.clear();
                after
                    .capabilities
                    .retain(|cap| !original.contains(&cap.to_string()));
                added_capabilities += after.capabilities.len();
                if !after.capabilities.is_empty() {
                    additions.push(after.to_string());
                }
            }
            (Some(Definable::Function(before)), Definable::Function(after)) => {
                ensure!(
                    before.signature.to_string() == after.signature.to_string()
                        && before
                            .annotations
                            .iter()
                            .map(ToString::to_string)
                            .collect::<BTreeSet<_>>()
                            == after.annotations.iter().map(ToString::to_string).collect(),
                    "Existing function signature or annotations changed: {key}"
                );
                if before.to_string() != after.to_string() {
                    replaced_functions.push(after.signature.ident.to_string());
                    // TypeQL permits exactly one redefinition per query.
                    redefine.push(format!("redefine {after}"));
                }
            }
            _ => anyhow::bail!("Definition kind changed: {key}"),
        }
    }
    let define = if additions.is_empty() {
        String::new()
    } else {
        format!("define\n{}", additions.join("\n"))
    };
    for query in std::iter::once(&define)
        .filter(|s| !s.is_empty())
        .chain(redefine.iter())
    {
        ensure!(
            typeql::parse_queries(query)?.len() == 1,
            "Migration must contain only the generated schema query"
        );
    }
    Ok(Plan {
        source_sha256: fingerprint(source)?,
        target_sha256: fingerprint(target)?,
        new_types,
        added_capabilities,
        new_functions,
        replaced_functions,
        define,
        redefine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_destructive_or_weakening_changes() {
        for (before, after) in [
            ("define entity item;", "define entity other;"),
            (
                "define attribute key, value string; entity item, owns key;",
                "define attribute key, value string; entity item;",
            ),
            (
                "define attribute key, value string;",
                "define attribute key, value integer;",
            ),
            (
                "define attribute key, value string; entity item, owns key @key;",
                "define attribute key, value string; entity item, owns key;",
            ),
        ] {
            assert!(plan(before, after).is_err());
        }
    }
    #[test]
    fn adds_definitions_and_capabilities_without_replacing_types() {
        let p = plan(
            "define entity item;",
            "define attribute title, value string; entity item, owns title;",
        )
        .unwrap();
        assert_eq!(p.new_types, 1);
        assert_eq!(p.added_capabilities, 1);
        assert!(p.redefine.is_empty());
        let parsed = definitions(&p.define).unwrap();
        let Definable::TypeDeclaration(item) = &parsed["type:item"] else {
            panic!("Missing type capability")
        };
        assert!(item.kind.is_none());
        assert_eq!(
            capabilities(item),
            BTreeSet::from(["owns title".to_owned()])
        );
    }
    #[test]
    fn identical_schema_is_a_noop_and_order_does_not_change_fingerprint() {
        let a = "define entity item; attribute title, value string;";
        let b = "define attribute title, value string; entity item;";
        assert_eq!(fingerprint(a).unwrap(), fingerprint(b).unwrap());
        let p = plan(a, b).unwrap();
        assert!(p.define.is_empty() && p.redefine.is_empty());
    }
    #[test]
    fn replaces_function_bodies_but_refuses_signature_changes() {
        let before = "define fun permitted($n: integer) -> boolean: match $n > 0; return check;";
        let after = "define fun permitted($n: integer) -> boolean: match $n > 1; return check;";
        let p = plan(before, after).unwrap();
        assert!(p.define.is_empty());
        assert_eq!(p.replaced_functions, vec!["permitted"]);
        assert_eq!(p.redefine.len(), 1);
        assert!(plan(
            before,
            "define fun permitted($n: double) -> boolean: match $n > 0; return check;"
        )
        .is_err());
    }
    #[test]
    fn rejects_ambiguous_partial_or_non_schema_input() {
        assert!(plan(
            "define entity item; item owns title;",
            "define entity item;"
        )
        .is_err());
        assert!(plan("match $x isa item; fetch {'x':$x};", "define entity item;").is_err());
    }
}
