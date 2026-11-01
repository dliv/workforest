//! Consumer-owned native baseline: engine profiles cannot silently widen it.
use crate::support;
use std::collections::BTreeSet;

// Snapshot of this product's adopted controls, not the union of consumer policies.
const REQUIRED_METHODS: &[&str] = &[
    "std::env::var",
    "std::env::var_os",
    "std::env::vars",
    "std::env::vars_os",
    "std::env::args",
    "std::env::args_os",
    "std::env::current_dir",
    "std::env::set_current_dir",
    "std::env::current_exe",
    "std::env::home_dir",
    "std::env::set_var",
    "std::env::remove_var",
    "std::process::exit",
    "std::process::abort",
    "std::process::Command::new",
    "std::io::stdout",
    "std::io::stderr",
    "std::io::stdin",
    "chrono::Utc::now",
    "chrono::Local::now",
    "std::time::SystemTime::now",
    "std::time::Instant::now",
    "ureq::get",
    "ureq::post",
    "ureq::put",
    "ureq::delete",
    "ureq::head",
    "ureq::options",
    "ureq::connect",
    "ureq::patch",
    "ureq::trace",
    "ureq::run",
    "ureq::Agent::get",
    "ureq::Agent::post",
    "ureq::Agent::put",
    "ureq::Agent::delete",
    "ureq::Agent::head",
    "ureq::Agent::options",
    "ureq::Agent::connect",
    "ureq::Agent::patch",
    "ureq::Agent::trace",
    "ureq::Agent::run",
];
const REQUIRED_MACROS: &[&str] = &[];
const REQUIRED_LINTS: &[(&str, &str, &str)] = &[
    ("clippy", "print_stdout", "warn"),
    ("clippy", "print_stderr", "warn"),
    ("clippy", "dbg_macro", "warn"),
];

fn configured_names(config: &toml::Value, key: &str) -> BTreeSet<String> {
    let Some(entries) = config.get(key) else {
        return BTreeSet::new();
    };
    let entries = entries.as_array().expect("native list must be an array");
    let names: BTreeSet<String> = entries
        .iter()
        .map(|value| {
            value
                .as_str()
                .or_else(|| value.get("path").and_then(toml::Value::as_str))
                .expect("native entry must name a path")
                .to_owned()
        })
        .collect();
    assert_eq!(
        names.len(),
        entries.len(),
        "duplicate native paths in {key}"
    );
    names
}

#[test]
fn native_controls_match_local_baseline() {
    let root = git_forest_policy::repo_root();
    let clippy: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("clippy.toml")).unwrap()).unwrap();
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    let methods = configured_names(&clippy, "disallowed-methods");
    let macros = configured_names(&clippy, "disallowed-macros");
    let mut problems = Vec::new();
    for name in REQUIRED_METHODS {
        if !methods.contains(*name) {
            problems.push(format!("missing native method control: {name}"));
        }
    }
    for name in REQUIRED_MACROS {
        if !macros.contains(*name) {
            problems.push(format!("missing native macro control: {name}"));
        }
    }
    for (group, name, expected) in REQUIRED_LINTS {
        let value = manifest
            .get("workspace")
            .and_then(|v| v.get("lints"))
            .and_then(|v| v.get(*group))
            .and_then(|v| v.get(*name));
        let level = value.and_then(|v| {
            v.as_str()
                .or_else(|| v.get("level").and_then(toml::Value::as_str))
        });
        if level != Some(*expected)
            && level != Some("forbid")
            && !(level == Some("deny") && *expected == "warn")
        {
            problems.push(format!(
                "native lint {group}.{name} must remain {expected} or stronger"
            ));
        }
    }
    // ArgsOs is a type; all permitted environment functions must be natively controlled.
    for name in support::ROOT_ALLOWED_ENV {
        if *name != "ArgsOs" && !methods.contains(&format!("std::env::{name}")) {
            problems.push(format!(
                "Root environment permission has no native control: std::env::{name}"
            ));
        }
    }
    assert!(problems.is_empty(), "native policy drift: {problems:?}");
}
