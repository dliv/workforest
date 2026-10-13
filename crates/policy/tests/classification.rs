//! Every Rust source file under src/ has exactly one class.

use std::collections::BTreeSet;
use std::path::Path;

use git_forest_policy::classification::{module_path, ModuleClass, MODULES};
use git_forest_policy::repo_root;

const SECTION: &str = "see docs/architecture.md, section \"Module classes\"";

fn collect_rust_files(root: &Path, dir: &Path, found: &mut BTreeSet<String>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("cannot read an entry of {}: {error}", dir.display()))
            .path();
        if path.is_dir() {
            collect_rust_files(root, &path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let relative = path
                .strip_prefix(root)
                .expect("files found under the repository root are inside it");
            found.insert(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
fn every_source_file_has_exactly_one_class() {
    let root = repo_root();
    let mut on_disk = BTreeSet::new();
    collect_rust_files(&root, &root.join("src"), &mut on_disk);

    let mut problems = Vec::new();
    let mut classified = BTreeSet::new();
    for (file, _) in MODULES {
        if !classified.insert((*file).to_owned()) {
            problems.push(format!(
                "{file} is listed in MODULES more than once; keep one entry"
            ));
        }
        if module_path(file).is_none() {
            problems.push(format!(
                "{file} in MODULES is not a path of the form src/<module>.rs"
            ));
        }
    }
    for file in on_disk.difference(&classified) {
        problems.push(format!(
            "{file} is not classified; add it to MODULES in crates/policy/src/classification.rs with its class"
        ));
    }
    for file in classified.difference(&on_disk) {
        problems.push(format!(
            "{file} is listed in MODULES but does not exist; remove the entry"
        ));
    }
    assert!(
        problems.is_empty(),
        "module classification is out of date ({SECTION}):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn at_least_one_pure_module_exists() {
    assert!(
        MODULES.iter().any(|(_, class)| *class == ModuleClass::Pure),
        "no module is classified Pure, so the purity check would scan nothing ({SECTION})"
    );
}
