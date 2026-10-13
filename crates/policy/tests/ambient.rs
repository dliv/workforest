//! Pure modules contain no ambient effects in their production code.

use git_forest_policy::classification::{ModuleClass, MODULES};
use git_forest_policy::repo_root;
use git_forest_policy::scan::ambient_violations;

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
