//! Pure modules contain no ambient effects in their production code.

use std::collections::BTreeSet;
use std::path::Path;

use git_forest_policy::classification::{ModuleClass, MODULES, PURE_ALLOWED_CRATES};
use git_forest_policy::manifest::dependency_crate_names;
use git_forest_policy::repo_root;
use git_forest_policy::scan::{ambient_violations, data_include_paths, external_crate_references};

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
            "env!",
            "option_env!",
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

/// Why an embedded file would make the build depend on something outside the
/// repository, or `None` when it resolves to an existing file inside it.
fn outside_repository(root: &Path, directory: &Path, included: &str) -> Option<String> {
    match directory.join(included).canonicalize() {
        Ok(resolved) if resolved.starts_with(root) => None,
        Ok(resolved) => Some(format!(
            "{included} resolves outside the repository, to {}",
            resolved.display()
        )),
        Err(error) => Some(format!("{included} cannot be resolved: {error}")),
    }
}

#[test]
fn pure_module_data_includes_stay_inside_the_repository() {
    let root = repo_root();
    let mut problems = Vec::new();
    for (file, class) in MODULES {
        if *class != ModuleClass::Pure {
            continue;
        }
        let path = root.join(file);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        let directory = path.parent().expect("a module file has a parent directory");
        for included in data_include_paths(&source) {
            if let Some(problem) = outside_repository(&root, directory, &included) {
                problems.push(format!("{file}: {problem}"));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "Pure modules may embed only files inside the repository ({SECTION}):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn data_include_paths_lists_only_relative_literals_in_production_code() {
    let source = r#"const A: &str = include_str!("../docs/a.md");
const B: &[u8] = include_bytes!("data/b.bin");
const C: &str = include_str!("/etc/hosts");
const E: &str = include_str!["e.md",];
const F: &[u8] = include_bytes! { "f.bin" };
const G: &str = include_str!("g\u{2e}md");
const H: &str = include_str!(r"h\i.md");
#[cfg(test)]
mod tests {
    const D: &str = include_str!("d.txt");
}
"#;
    assert_eq!(
        data_include_paths(source),
        vec!["../docs/a.md", "e.md", "data/b.bin", "f.bin"]
    );
}

#[test]
fn repository_containment_check_flags_escapes_and_missing_files() {
    let root = repo_root();
    let src = root.join("src");
    assert_eq!(outside_repository(&root, &src, "../Cargo.toml"), None);
    assert!(outside_repository(&root, &src, "../../../../../../../../../../etc/hosts").is_some());
    assert!(outside_repository(&root, &src, "no-such-file.txt").is_some());
}

/// The dependency names of the root package, read from the root `Cargo.toml`.
fn root_dependencies() -> BTreeSet<String> {
    let manifest = std::fs::read_to_string(repo_root().join("Cargo.toml"))
        .unwrap_or_else(|error| panic!("cannot read the root Cargo.toml: {error}"));
    dependency_crate_names(&manifest)
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn pure_modules_use_only_allowed_crates() {
    let dependencies = root_dependencies();
    let mut used = BTreeSet::new();
    let mut problems = Vec::new();
    for (file, class) in MODULES {
        if *class != ModuleClass::Pure {
            continue;
        }
        let source = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for name in external_crate_references(&source, &dependencies) {
            if !PURE_ALLOWED_CRATES.contains(&name.as_str()) {
                problems.push(format!("{file}: crate {name}"));
            }
            used.insert(name);
        }
    }
    for name in PURE_ALLOWED_CRATES {
        if !used.contains(*name) {
            problems.push(format!(
                "{name} is in PURE_ALLOWED_CRATES but no Pure module uses it; remove the entry"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "Pure modules may use only the dependency crates in PURE_ALLOWED_CRATES \
         (crates/policy/src/classification.rs); move the call into a Shared or Root module, \
         or allow the crate there with a reason ({SECTION}):\n  {}",
        problems.join("\n  ")
    );
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
