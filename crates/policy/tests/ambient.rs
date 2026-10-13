//! Pure modules contain no ambient effects in their production code.

use std::path::Path;

use git_forest_policy::classification::{ModuleClass, MODULES};
use git_forest_policy::repo_root;
use git_forest_policy::scan::{ambient_violations, data_include_paths};

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
