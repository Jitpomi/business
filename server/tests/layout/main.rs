//! Prevent dormant source and test files from silently escaping Cargo coverage.
use anyhow::{ensure, Result};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
fn rust_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, output)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            output.push(path.canonicalize()?);
        }
    }
    Ok(())
}
fn visit(file: PathBuf, seen: &mut HashSet<PathBuf>) -> Result<()> {
    let file = file.canonicalize()?;
    if !seen.insert(file.clone()) {
        return Ok(());
    }
    let source = std::fs::read_to_string(&file)?;
    let modules = regex::Regex::new(
        r#"(?m)(?:#\[path\s*=\s*"([^"]+)"\]\s*)?(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;"#,
    )?;
    for capture in modules.captures_iter(&source) {
        let next = if let Some(path) = capture.get(1) {
            file.parent().unwrap().join(path.as_str())
        } else {
            let parent = file.parent().unwrap();
            let directory = if matches!(
                file.file_stem().and_then(|s| s.to_str()),
                Some("lib" | "main" | "mod")
            ) {
                parent.to_path_buf()
            } else {
                parent.join(file.file_stem().unwrap())
            };
            let name = &capture[2];
            let plain = directory.join(format!("{name}.rs"));
            if plain.exists() {
                plain
            } else {
                directory.join(name).join("mod.rs")
            }
        };
        ensure!(
            next.exists(),
            "Declared module does not exist: {}",
            next.display()
        );
        visit(next, seen)?;
    }
    Ok(())
}
#[test]
fn every_rust_source_and_test_is_connected_to_a_cargo_target() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut seen = HashSet::new();
    visit(root.join("src/lib.rs"), &mut seen)?;
    visit(root.join("src/main.rs"), &mut seen)?;
    for directory in ["src/bin", "tests"] {
        for entry in std::fs::read_dir(root.join(directory))? {
            let path = entry?.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "rs") {
                visit(path, &mut seen)?;
            } else if directory == "tests" && path.join("main.rs").exists() {
                visit(path.join("main.rs"), &mut seen)?;
            }
        }
    }
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files)?;
    rust_files(&root.join("tests"), &mut files)?;
    let missing: Vec<_> = files
        .iter()
        .filter(|f| !seen.contains(*f))
        .map(|f| f.strip_prefix(root).unwrap().display().to_string())
        .collect();
    ensure!(
        missing.is_empty(),
        "Rust files outside the active module graph: {missing:?}"
    );
    Ok(())
}
#[test]
fn every_service_uses_the_common_dogrs_modules() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/services");
    fn walk(directory: &Path) -> Result<()> {
        let entries: Vec<_> =
            std::fs::read_dir(directory)?.collect::<std::result::Result<_, _>>()?;
        let prefix = directory
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .replace('-', "_");
        if directory.join(format!("{prefix}_service.rs")).exists() {
            for suffix in ["service", "shared", "hooks", "schema"] {
                ensure!(
                    directory.join(format!("{prefix}_{suffix}.rs")).is_file(),
                    "Missing {prefix}_{suffix}.rs"
                );
            }
            ensure!(directory.join("mod.rs").is_file(), "Missing service mod.rs");
        }
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path)?;
            }
        }
        Ok(())
    }
    // Every registered service must be a directory, so a flat file cannot silently
    // evade the common-module check.
    for entry in std::fs::read_dir(&root)? {
        let path = entry?.path();
        if path.is_file() {
            ensure!(
                matches!(
                    path.file_name().and_then(|n| n.to_str()),
                    Some("mod.rs" | "types.rs")
                ),
                "Flat service module: {}",
                path.display()
            );
        } else if path.is_dir() {
            let name = path.file_name().unwrap().to_str().unwrap();
            ensure!(
                path.join(format!("{name}_service.rs")).is_file(),
                "Missing service implementation: {name}"
            );
        }
    }
    let src = root.parent().unwrap();
    let lib = std::fs::read_to_string(src.join("lib.rs"))?;
    for name in ["app", "hooks", "channels", "services"] {
        ensure!(
            src.join(format!("{name}.rs")).is_file() || src.join(name).join("mod.rs").is_file(),
            "Missing application module: {name}"
        );
        ensure!(
            regex::Regex::new(&format!(r"\bmod\s+{name}\s*;"))?.is_match(&lib),
            "Undeclared application module: {name}"
        );
    }
    walk(&root)?;
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/plugins"))
}

#[test]
fn business_rules_do_not_import_vendor_protocols_or_credentials() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["src/finance", "src/billing"] {
        rust_files(&root.join(dir), &mut files)?;
    }
    files.push(root.join("src/services/types.rs"));
    rust_files(&root.join("src/services/financial"), &mut files)?;
    for file in files {
        let source = std::fs::read_to_string(&file)?;
        for forbidden in [
            "Provider::Stripe",
            "Provider::Mercury",
            "STRIPE_",
            "MERCURY_",
            "RESEND_",
            "plugins::stripe",
            "plugins::mercury",
            "reqwest::Client",
            "reqwest::Method",
            "bill_stripe_id",
            "bill_mercury_id",
        ] {
            ensure!(
                !source.contains(forbidden),
                "Vendor dependency {forbidden} in {}",
                file.display()
            );
        }
    }
    Ok(())
}

#[test]
fn generic_automation_does_not_import_builtin_provider_packages() -> Result<()> {
    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/automation"),
        &mut files,
    )?;
    for file in files {
        let source = std::fs::read_to_string(&file)?;
        for forbidden in ["crate::plugins::", "STRIPE_", "MERCURY_", "RESEND_"] {
            ensure!(
                !source.contains(forbidden),
                "Provider package dependency {forbidden} in {}",
                file.display()
            );
        }
    }
    Ok(())
}
