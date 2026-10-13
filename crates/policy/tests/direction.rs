//! Crate-internal imports follow the class rules, or a sanctioned allowlist
//! entry that is still in use.

use std::collections::BTreeSet;

use git_forest_policy::classification::{
    class_allows, module_path, ModuleClass, ALLOWED_EDGES, MODULES,
};
use git_forest_policy::repo_root;
use git_forest_policy::scan::crate_edges;

const SECTION: &str = "see docs/architecture.md, section \"Import direction\"";

fn known_modules() -> Vec<String> {
    MODULES
        .iter()
        .filter_map(|(file, _)| module_path(file))
        .collect()
}

fn class_of(module: &str) -> ModuleClass {
    MODULES
        .iter()
        .find(|(file, _)| module_path(file).as_deref() == Some(module))
        .map(|(_, class)| *class)
        .unwrap_or_else(|| panic!("{module} is not a classified module ({SECTION})"))
}

fn class_rule(class: ModuleClass) -> &'static str {
    match class {
        ModuleClass::Root => "Root modules may import anything",
        ModuleClass::Shell => "Shell modules import only Shared and Pure",
        ModuleClass::Command => {
            "Command modules import only Shared, Pure and allowlisted Command modules"
        }
        ModuleClass::Shared => "Shared modules import only Shared and Pure",
        ModuleClass::Pure => "Pure modules import only Pure and allowlisted edges",
        ModuleClass::TestSupport => "TestSupport modules are not scanned",
    }
}

/// Every crate-internal import edge of every scanned module, as (from, to).
fn production_edges() -> BTreeSet<(String, String)> {
    let known = known_modules();
    let mut edges = BTreeSet::new();
    for (file, class) in MODULES {
        if *class == ModuleClass::TestSupport {
            continue;
        }
        let module = module_path(file).expect("every MODULES entry has a module path");
        let source = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for target in crate_edges(&module, &source, &known) {
            edges.insert((module.clone(), target));
        }
    }
    edges
}

fn is_allowlisted(from: &str, to: &str) -> bool {
    ALLOWED_EDGES
        .iter()
        .any(|edge| edge.from == from && edge.to == to)
}

#[test]
fn every_import_follows_the_class_rules_or_the_allowlist() {
    let violations: Vec<String> = production_edges()
        .iter()
        .filter(|(from, to)| {
            !class_allows(class_of(from), class_of(to)) && !is_allowlisted(from, to)
        })
        .map(|(from, to)| {
            let from_class = class_of(from);
            let to_class = class_of(to);
            format!(
                "{from} -> {to}: a {from_class:?} module imports a {to_class:?} module, but {}. \
                 Fix: move the import, or add an ALLOWED_EDGES entry with a reason in \
                 crates/policy/src/classification.rs; {SECTION}.",
                class_rule(from_class)
            )
        })
        .collect();
    assert!(
        violations.is_empty(),
        "import direction violations:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn every_allowlisted_edge_is_still_imported() {
    let edges = production_edges();
    let stale: Vec<String> = ALLOWED_EDGES
        .iter()
        .filter(|edge| !edges.contains(&(edge.from.to_owned(), edge.to.to_owned())))
        .map(|edge| {
            format!(
                "{} -> {} is allowlisted but no longer imported; remove its ALLOWED_EDGES entry ({SECTION})",
                edge.from, edge.to
            )
        })
        .collect();
    assert!(
        stale.is_empty(),
        "stale allowlist entries:\n  {}",
        stale.join("\n  ")
    );
}

#[test]
fn the_scan_sees_the_crate_internal_imports() {
    let count = production_edges().len();
    assert!(
        count >= 10,
        "only {count} crate-internal import edges found; the direction scan is not seeing the imports ({SECTION})"
    );
}

#[test]
fn planted_paths_resolve_to_exactly_the_real_edges() {
    let source = include_str!("fixtures/direction-planted.rs");
    let edges = crate_edges("commands::rm", source, &known_modules());
    let expected: BTreeSet<String> = ["commands::branch_state", "forest", "meta", "paths"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();
    assert_eq!(edges, expected);
}

fn module_set(modules: &[&str]) -> BTreeSet<String> {
    modules.iter().map(|module| (*module).to_owned()).collect()
}

#[test]
fn nested_super_paths_resolve_once() {
    assert_eq!(
        crate_edges(
            "commands::rm",
            "const _: &str = super::super::channel::APP_NAME;",
            &known_modules()
        ),
        module_set(&["channel"])
    );
}

#[test]
fn inline_module_parent_references_are_self_edges() {
    let source = r#"pub const APP_NAME: &str = "x";
pub mod child {
    pub fn name() -> &'static str {
        super::APP_NAME
    }
    pub fn same() -> &'static str {
        self::name()
    }
}
"#;
    assert_eq!(
        crate_edges("channel", source, &known_modules()),
        module_set(&[])
    );
}

#[test]
fn inline_module_super_chains_resolve_against_their_scope() {
    assert_eq!(
        crate_edges(
            "commands::rm",
            "mod inner { fn f() { let _ = super::super::branch_state::X; } }",
            &known_modules()
        ),
        module_set(&["commands::branch_state"])
    );
}

#[test]
fn use_trees_inside_inline_modules_resolve_against_their_scope() {
    assert_eq!(
        crate_edges(
            "channel",
            "mod inner { use super::super::meta::ForestMode; }",
            &known_modules()
        ),
        module_set(&["meta"])
    );
    assert_eq!(
        crate_edges(
            "commands::rm",
            "mod inner { use super::plan_rm; }",
            &known_modules()
        ),
        module_set(&[])
    );
}
