//! Pure modules contain none of the effects the purity scan lists and use only allowlisted
//! dependency crates of their own package; Root modules contain none of the forms the Root
//! profile forbids.

use std::collections::BTreeSet;

use git_forest_policy::classification::{
    crates_allowed_in_pure_modules, module_id, ModuleClass, MODULES, PURE_ALLOWED_CRATES,
};
use git_forest_policy::edges::{CrateClass, CRATES};
use git_forest_policy::manifest::dependency_crate_names;
use git_forest_policy::repo_root;
use git_forest_policy::scan::{
    ambient_violations, external_crate_references, root_ambient_violations,
};

const SECTION: &str = "see docs/architecture.md, section \"Pure modules\"";
const ROOT_SECTION: &str = "see docs/architecture.md, section \"Root effects\"";

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
         Shared module of an effectful crate, or reclassify the module ({SECTION}):\n  {}",
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

#[test]
fn root_modules_have_no_forbidden_effects() {
    let mut scanned = 0;
    let mut problems = Vec::new();
    for (file, class) in MODULES {
        if *class != ModuleClass::Root {
            continue;
        }
        scanned += 1;
        let source = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for violation in root_ambient_violations(&source) {
            problems.push(format!("{file}: {violation}"));
        }
    }
    assert!(scanned > 0, "no Root module was scanned ({ROOT_SECTION})");
    assert!(
        problems.is_empty(),
        "Root modules compose: they may not touch the filesystem, the network, threads or the \
         environment beyond the process arguments; move the effect into a Shared module of an \
         effectful crate and pass the data in ({ROOT_SECTION}):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn root_profile_reports_forbidden_forms() {
    // Each source reports exactly its violation.
    let exact = [
        (
            r#"fn f() { let _ = std::fs::read_to_string("x"); }"#,
            "std::fs",
        ),
        (
            r#"use std::fs as disk; fn f() { let _ = disk::read("x"); }"#,
            "std::fs",
        ),
        ("use std::{fs, io}; fn f() {}", "std::fs"),
        (
            r#"fn f() { let _ = std::net::TcpStream::connect("x"); }"#,
            "std::net",
        ),
        (
            "fn f() { let _ = std::thread::spawn(|| {}); }",
            "std::thread",
        ),
        ("fn f() { let _ = std::env::temp_dir(); }", "std::env"),
        ("fn f() { let _ = std::env::current_dir(); }", "std::env"),
        (
            r#"use std::env::var as read_var; fn f() { let _ = read_var("HOME"); }"#,
            "std::env",
        ),
        (
            r#"use std::env; fn f() { let _ = env::var("HOME"); }"#,
            "std::env",
        ),
        (
            "fn f(p: &std::path::Path) -> bool { p.exists() }",
            "path effect exists",
        ),
    ];
    for (source, violation) in exact {
        assert_eq!(root_ambient_violations(source), vec![violation], "{source}");
    }
    // The scan escapes report at least their own violation; they may also trip the shared
    // escape rules.
    let escapes = [
        (r#"include!("x.rs");"#, "include!"),
        (r#"#[path = "x.rs"] mod m;"#, "#[path]"),
        ("fn f() { unsafe {} }", "unsafe"),
        ("use std::*;", "std::*"),
    ];
    for (source, violation) in escapes {
        let violations = root_ambient_violations(source);
        assert!(
            violations.iter().any(|reported| reported == violation),
            "{source}: expected {violation:?} among {violations:?}"
        );
    }
}

/// Process, clock and stream uses pass the Root profile because Clippy bans them outside
/// their sanctioned sites, which the positive control counts.
#[test]
fn root_profile_permits_sanctioned_forms() {
    let sources = [
        "fn f() { let _ = std::env::args_os(); }",
        "use std::env::args_os; fn f() { let _ = args_os(); }",
        "use std::env::args_os as argv; fn f() { let _ = argv(); }",
        "use std::env; fn f() { let _ = env::args_os(); }",
        "use std::env::ArgsOs; fn f(_: ArgsOs) {}",
        r#"fn f() {
    let _ = std::process::Command::new("brew");
    let _ = std::process::ExitCode::SUCCESS;
    let _ = std::io::stdout();
    let _ = std::time::Duration::from_secs(1);
    let _ = include_str!("x.md");
    let _ = env!("CARGO_PKG_VERSION");
}"#,
    ];
    for source in sources {
        assert_eq!(
            root_ambient_violations(source),
            Vec::<String>::new(),
            "{source}"
        );
    }
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
        "git_forest_core",
        "git_forest_ops",
        "git_forest_update_check",
        "serde",
        "serde_json",
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
