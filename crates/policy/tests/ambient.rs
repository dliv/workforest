//! Pure modules contain none of the effects the purity scan lists and use only allowlisted
//! dependency crates of their own package.

use std::collections::BTreeSet;

use git_forest_policy::classification::{
    crates_allowed_in_pure_modules, module_id, ModuleClass, MODULES, PURE_ALLOWED_CRATES,
};
use git_forest_policy::edges::{CrateClass, CRATES};
use git_forest_policy::manifest::dependency_crate_names;
use git_forest_policy::repo_root;
use git_forest_policy::scan::{ambient_violations, external_crate_references};

const SECTION: &str = "see docs/architecture.md, section \"Pure modules\"";

#[test]
fn pure_modules_have_no_ambient_effects() {
    let mut scanned = 0;
    let mut problems = Vec::new();
    for (file, class) in MODULES {
        if *class != ModuleClass::Pure {
            continue;
        }
        scanned += 1;
        let source = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for violation in ambient_violations(&source) {
            problems.push(format!("{file}: {violation}"));
        }
    }
    assert!(scanned > 0, "no Pure module was scanned ({SECTION})");
    assert!(
        problems.is_empty(),
        "Pure modules must not read or change the outside world; move the effect into a \
         Shared or Root module, or reclassify the module ({SECTION}):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn planted_effects_are_all_reported() {
    let source = include_str!("fixtures/ambient-planted.rs");
    assert_eq!(
        ambient_violations(source),
        vec![
            "std::*",
            "std::env",
            "std::fs",
            "std::net",
            "std::process",
            "std::thread",
            "std::time",
        ]
    );
}

/// The dependency names of the package at `crate_dir` (empty for the root package), read from
/// its `Cargo.toml`.
fn package_dependencies(crate_dir: &str) -> BTreeSet<String> {
    let path = repo_root().join(crate_dir).join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    dependency_crate_names(&manifest)
}

fn root_dependencies() -> BTreeSet<String> {
    package_dependencies("")
}

/// The dependency crates `source` names that are not in `allowed`.
fn disallowed_crates(
    source: &str,
    dependencies: &BTreeSet<String>,
    allowed: &BTreeSet<String>,
) -> BTreeSet<String> {
    external_crate_references(source, dependencies)
        .difference(allowed)
        .cloned()
        .collect()
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn pure_modules_use_only_allowed_crates() {
    let allowed = crates_allowed_in_pure_modules(CRATES);
    let mut used = BTreeSet::new();
    let mut problems = Vec::new();
    for (file, class) in MODULES {
        if *class != ModuleClass::Pure {
            continue;
        }
        let crate_dir = module_id(file)
            .expect("every MODULES entry has a module identity")
            .crate_dir;
        let dependencies = package_dependencies(&crate_dir);
        let source = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for name in disallowed_crates(&source, &dependencies, &allowed) {
            problems.push(format!("{file}: crate {name}"));
        }
        used.extend(external_crate_references(&source, &dependencies));
    }
    for (name, _) in PURE_ALLOWED_CRATES {
        if !used.contains(*name) {
            problems.push(format!(
                "{name} is in PURE_ALLOWED_CRATES but no Pure module uses it; remove the entry"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "Pure modules may use only the dependency crates in PURE_ALLOWED_CRATES \
         (crates/policy/src/classification.rs) and Pure workspace crates; move the call into a \
         Shared or Root module, or allow the crate there with a reason ({SECTION}):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn pure_member_crates_are_allowed_in_pure_modules() {
    let crates: &[(&str, CrateClass)] = &[
        ("app", CrateClass::Binary),
        ("tools", CrateClass::Tooling),
        ("widgets-core", CrateClass::Pure { feature: "widgets" }),
    ];
    let allowed = crates_allowed_in_pure_modules(crates);
    let dependencies = names(&["anyhow", "ureq", "widgets_core"]);
    let source = r#"use widgets_core::Widget;
fn f() -> anyhow::Result<()> {
    let _ = ureq::get("x");
    Ok(())
}
"#;
    assert_eq!(
        disallowed_crates(source, &dependencies, &allowed),
        names(&["ureq"])
    );
    assert!(allowed.contains("widgets_core"));
    assert!(!allowed.contains("app") && !allowed.contains("tools"));
}

#[test]
fn root_manifest_dependencies_are_read() {
    let found = root_dependencies();
    let missing: Vec<&str> = [
        "anyhow",
        "chrono",
        "clap",
        "directories",
        "libc",
        "semver",
        "serde",
        "serde_json",
        "toml",
        "ureq",
    ]
    .into_iter()
    .filter(|name| !found.contains(*name))
    .collect();
    assert!(
        missing.is_empty(),
        "the dependency scan of the root Cargo.toml missed {}; without them the crate \
         allowlist checks nothing ({SECTION})",
        missing.join(", ")
    );
}

#[test]
fn dependency_scan_reads_only_dependency_tables() {
    let manifest = r#"[package]
name = "demo"
version = "1.0.0"

[ dependencies ] # main table
plain = "1"
with-hyphen = { version = "1", features = ["x"] }
"quoted-name" = "1"
dotted.workspace = true
# commented = "1"

[dependencies.table-form]
version = "1"

[target.'cfg(unix)'.dependencies]
unix-only = "1"

[target.'cfg(windows)'.dependencies.win-table]
version = "1"

[dev-dependencies]
dev-only = "1"

[build-dependencies]
build-only = "1"

[[bin]]
name = "demo"

[features]
default = []
"#;
    assert_eq!(
        dependency_crate_names(manifest),
        names(&[
            "dotted",
            "plain",
            "quoted_name",
            "table_form",
            "unix_only",
            "win_table",
            "with_hyphen",
        ])
    );
}

#[test]
fn external_crate_references_are_found_exactly() {
    let dependencies = names(&[
        "anyhow",
        "chrono",
        "clap",
        "directories",
        "libc",
        "semver",
        "serde_json",
        "toml",
        "ureq",
    ]);
    let source = r#"use clap::Parser;
use directories as dirs;
use std::path::Path;
#[derive(clap::Subcommand)]
enum E {
    A,
}
fn a() {
    let _ = ::chrono::Utc::now();
}
fn b() -> serde_json::Value {
    serde_json::json!(null)
}
fn c(f: Format) -> bool {
    matches!(f, Format::toml)
}
fn d() {
    let _ = dirs::UserDirs::new();
}
fn e() -> bool {
    match ::semver::Version::parse("1.0.0") {
        _ => true,
    }
}
fn g() -> ::anyhow::Result<()> {
    todo!()
}
fn h(x: Option<u8>) -> u32 {
    match x {
        Some(_) => ::libc::getpid() as u32,
        None => 0,
    }
}
// ureq::get("x") in a comment
const S: &str = "ureq::get";
#[cfg(test)]
mod tests {
    fn t() {
        let _ = ureq::get("x");
    }
}
"#;
    assert_eq!(
        external_crate_references(source, &dependencies),
        names(&[
            "anyhow",
            "chrono",
            "clap",
            "directories",
            "libc",
            "semver",
            "serde_json",
        ])
    );
}
