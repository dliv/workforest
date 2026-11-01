//! Crate-internal imports follow the class rules, or a sanctioned allowlist
//! entry that is still in use. Edges are computed within each crate.

use std::collections::BTreeSet;

use crate::support::classification::{
    class_allows, crate_modules, module_id, ModuleClass, ModuleId, ALLOWED_EDGES, MODULES,
};
use arch_gates::graph::{check_direction, crate_edges, DirectionViolation, Edge};
use git_forest_policy::repo_root;

const SECTION: &str = "see docs/architecture.md, section \"Import direction\"";

/// A fixed module list for the scanner tests, so that they do not depend on which crate holds
/// which module.
const SCANNER_MODULES: &[&str] = &[
    "crate",
    "channel",
    "cli",
    "commands",
    "commands::branch_state",
    "commands::reset",
    "commands::rm",
    "config",
    "forest",
    "git",
    "meta",
    "paths",
    "plan",
    "plan::rm",
    "version_check",
];

fn known_modules() -> Vec<String> {
    SCANNER_MODULES
        .iter()
        .map(|module| (*module).to_owned())
        .collect()
}

fn class_of(crate_dir: &str, module: &str) -> ModuleClass {
    MODULES
        .iter()
        .find(|(file, _)| {
            module_id(file).is_some_and(|id| id.crate_dir == crate_dir && id.path == module)
        })
        .map(|(_, class)| *class)
        .unwrap_or_else(|| {
            panic!(
                "{} is not a classified module ({SECTION})",
                label(crate_dir, module)
            )
        })
}

/// A module as messages name it: its path, and its crate's directory outside the root package.
fn label(crate_dir: &str, module: &str) -> String {
    if crate_dir.is_empty() {
        module.to_owned()
    } else {
        format!("{module} (in {crate_dir})")
    }
}

/// Every crate-internal import edge of every scanned module, as (crate directory, from, to).
fn production_edges() -> BTreeSet<Edge> {
    let mut edges = BTreeSet::new();
    for (file, class) in MODULES {
        if *class == ModuleClass::TestSupport {
            continue;
        }
        let ModuleId { crate_dir, path } =
            module_id(file).expect("every MODULES entry has a module identity");
        let known = crate_modules(MODULES, &crate_dir);
        let source = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for target in crate_edges(&path, &source, &known) {
            edges.insert(Edge {
                crate_dir: crate_dir.clone(),
                from: path.clone(),
                to: target,
            });
        }
    }
    edges
}

fn direction_problems() -> Vec<DirectionViolation> {
    let exceptions: Vec<Edge> = ALLOWED_EDGES
        .iter()
        .map(|edge| {
            assert!(
                !edge.reason.trim().is_empty(),
                "allowed edges require a reason"
            );
            Edge {
                crate_dir: edge.crate_dir.to_owned(),
                from: edge.from.to_owned(),
                to: edge.to.to_owned(),
            }
        })
        .collect();
    check_direction(&production_edges(), &exceptions, |edge| {
        class_allows(
            class_of(&edge.crate_dir, &edge.from),
            class_of(&edge.crate_dir, &edge.to),
        )
    })
}

#[test]
fn every_import_follows_the_class_rules_or_the_allowlist() {
    let violations: Vec<_> = direction_problems()
        .into_iter()
        .filter(|problem| !matches!(problem, DirectionViolation::Stale(_)))
        .collect();
    assert!(
        violations.is_empty(),
        "import direction violations ({SECTION}): {violations:?}"
    );
}

#[test]
fn every_allowlisted_edge_is_still_imported() {
    let stale: Vec<_> = direction_problems()
        .into_iter()
        .filter(|problem| matches!(problem, DirectionViolation::Stale(_)))
        .collect();
    assert!(
        stale.is_empty(),
        "stale allowlist entries ({SECTION}): {stale:?}"
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
fn paths_resolve_within_their_own_crate() {
    let modules: &[(&str, ModuleClass)] = &[
        ("src/store.rs", ModuleClass::Shared),
        ("src/lib.rs", ModuleClass::Root),
        ("src/model.rs", ModuleClass::Shared),
        ("crates/widgets/core/src/draw/mod.rs", ModuleClass::Pure),
        ("crates/widgets/core/src/lib.rs", ModuleClass::Pure),
        ("crates/widgets/core/src/model.rs", ModuleClass::Pure),
    ];
    let known = crate_modules(modules, "crates/widgets/core");
    assert_eq!(known, vec!["draw", "crate", "model"]);
    assert_eq!(
        crate_edges(
            "draw",
            "use crate::model::Widget;\nfn f() { let _ = crate::store::find(); }",
            &known
        ),
        module_set(&["crate", "model"])
    );
    assert_eq!(crate_modules(modules, ""), vec!["store", "crate", "model"]);
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

#[test]
fn grouped_use_prefixes_add_no_edge_of_their_own() {
    assert_eq!(
        crate_edges(
            "commands::rm",
            "use super::super::{channel::APP_NAME, meta::ForestMode};",
            &known_modules()
        ),
        module_set(&["channel", "meta"])
    );
    assert_eq!(
        crate_edges(
            "commands::rm",
            "mod inner { use super::super::super::{channel::APP_NAME, meta::ForestMode}; }",
            &known_modules()
        ),
        module_set(&["channel", "meta"])
    );
}
