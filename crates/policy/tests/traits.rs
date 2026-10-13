//! Production code declares no traits unless one is sanctioned here.

use std::collections::BTreeSet;

use git_forest_policy::classification::{module_path, ModuleClass, MODULES};
use git_forest_policy::repo_root;
use git_forest_policy::scan::trait_declarations;

const SECTION: &str = "see docs/architecture.md, section \"No production traits\"";

/// Sanctioned production traits as (module, trait, reason). None today.
const ALLOWED_TRAITS: &[(&str, &str, &str)] = &[];

#[test]
fn production_code_declares_no_unsanctioned_traits() {
    let mut declared = BTreeSet::new();
    for (file, class) in MODULES {
        if *class == ModuleClass::TestSupport {
            continue;
        }
        let module = module_path(file).expect("every MODULES entry has a module path");
        let source = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for name in trait_declarations(&source) {
            declared.insert((module.clone(), name));
        }
    }
    let allowed: BTreeSet<(String, String)> = ALLOWED_TRAITS
        .iter()
        .map(|(module, name, _)| ((*module).to_owned(), (*name).to_owned()))
        .collect();

    let mut problems = Vec::new();
    for (module, name) in declared.difference(&allowed) {
        problems.push(format!(
            "Declared but not allowed: {module} declares trait {name}; use a concrete type or an \
             enum, or add it to ALLOWED_TRAITS in crates/policy/tests/traits.rs with a reason"
        ));
    }
    for (module, name) in allowed.difference(&declared) {
        problems.push(format!(
            "Allowed but no longer declared: {module} trait {name}; remove its ALLOWED_TRAITS entry"
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
