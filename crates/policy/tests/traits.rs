//! Production code declares no traits unless one is sanctioned here.

use std::collections::BTreeSet;

use git_forest_policy::classification::{module_id, ModuleClass, ModuleId, MODULES};
use git_forest_policy::repo_root;
use git_forest_policy::scan::trait_declarations;

const SECTION: &str = "see docs/architecture.md, section \"No production traits\"";

/// Sanctioned production traits as (crate directory, module, trait, reason), the crate
/// directory empty for the root package. None today.
const ALLOWED_TRAITS: &[(&str, &str, &str, &str)] = &[];

/// A module as messages name it: its path, and its crate's directory outside the root package.
fn label(crate_dir: &str, module: &str) -> String {
    if crate_dir.is_empty() {
        module.to_owned()
    } else {
        format!("{module} (in {crate_dir})")
    }
}

#[test]
fn production_code_declares_no_unsanctioned_traits() {
    let mut declared = BTreeSet::new();
    for (file, class) in MODULES {
        if *class == ModuleClass::TestSupport {
            continue;
        }
        let ModuleId { crate_dir, path } =
            module_id(file).expect("every MODULES entry has a module identity");
        let source = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for name in trait_declarations(&source) {
            declared.insert((crate_dir.clone(), path.clone(), name));
        }
    }
    let allowed: BTreeSet<(String, String, String)> = ALLOWED_TRAITS
        .iter()
        .map(|(crate_dir, module, name, _)| {
            (
                (*crate_dir).to_owned(),
                (*module).to_owned(),
                (*name).to_owned(),
            )
        })
        .collect();

    let mut problems = Vec::new();
    for (crate_dir, module, name) in declared.difference(&allowed) {
        problems.push(format!(
            "Declared but not allowed: {} declares trait {name}; use a concrete type or an \
             enum, or add it to ALLOWED_TRAITS in crates/policy/tests/traits.rs with a reason",
            label(crate_dir, module)
        ));
    }
    for (crate_dir, module, name) in allowed.difference(&declared) {
        problems.push(format!(
            "Allowed but no longer declared: {} trait {name}; remove its ALLOWED_TRAITS entry",
            label(crate_dir, module)
        ));
    }
    assert!(
        problems.is_empty(),
        "production traits ({SECTION}):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn planted_trait_declarations_are_found_exactly() {
    let source = include_str!("fixtures/trait-planted.rs");
    assert_eq!(trait_declarations(source), vec!["A", "B", "C", "D"]);
}
