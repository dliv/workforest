//! Every Rust source file under the `src/` directory of every product crate has exactly one
//! class, and each crate holds only the module classes its crate class allows: a Pure crate
//! only Pure files, the Binary crate only Root and Pure files, and only the Binary crate Root
//! files.

use std::collections::BTreeSet;
use std::path::Path;

use git_forest_policy::classification::{
    crate_holds_module_class, module_id, ModuleClass, ModuleId, MODULES,
};
use git_forest_policy::edges::{CrateClass, CRATES};
use git_forest_policy::metadata::{workspace_members, Member};
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

fn class_of_crate(name: &str) -> Option<CrateClass> {
    CRATES
        .iter()
        .find(|(crate_name, _)| *crate_name == name)
        .map(|(_, class)| *class)
}

#[test]
fn every_source_file_has_exactly_one_class() {
    let root = repo_root();
    let members = workspace_members().unwrap_or_else(|error| panic!("{error} ({SECTION})"));
    // Every member that is not classified Tooling holds product code, an unclassified one too.
    let product: Vec<&Member> = members
        .iter()
        .filter(|member| class_of_crate(&member.name) != Some(CrateClass::Tooling))
        .collect();
    assert!(
        product.iter().any(|member| member.dir.is_empty()),
        "the root package is not among the product crates, so its src/ would go unwalked \
         ({SECTION})"
    );
    let mut on_disk = BTreeSet::new();
    for member in &product {
        collect_rust_files(&root, &root.join(&member.dir).join("src"), &mut on_disk);
    }

    let mut problems = Vec::new();
    let mut classified = BTreeSet::new();
    for (file, class) in MODULES {
        if !classified.insert((*file).to_owned()) {
            problems.push(format!(
                "{file} is listed in MODULES more than once; keep one entry"
            ));
        }
        let Some(ModuleId { crate_dir, .. }) = module_id(file) else {
            problems.push(format!(
                "{file} in MODULES is not a path of the form [<crate directory>/]src/<module>.rs"
            ));
            continue;
        };
        let Some(member) = product.iter().find(|member| member.dir == crate_dir) else {
            problems.push(format!(
                "{file} in MODULES is not under the src/ directory of a product crate; remove \
                 the entry, or add its crate to the workspace"
            ));
            continue;
        };
        // A member missing from CRATES has no crate class; the edges test reports it.
        let Some(crate_class) = class_of_crate(&member.name) else {
            continue;
        };
        if !crate_holds_module_class(crate_class, *class) {
            problems.push(module_class_problem(
                file,
                &member.name,
                crate_class,
                *class,
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

/// Why a file of the crate `crate_name`, of class `crate_class`, may not be classified
/// `module_class`, and how to fix it.
fn module_class_problem(
    file: &str,
    crate_name: &str,
    crate_class: CrateClass,
    module_class: ModuleClass,
) -> String {
    match crate_class {
        CrateClass::Pure { .. } => format!(
            "{file} belongs to the Pure crate {crate_name} and must be classified Pure, not \
             {module_class:?}"
        ),
        CrateClass::Binary => format!(
            "{file} belongs to the Binary crate {crate_name}, which holds only Root and Pure \
             modules: move the code into a library crate, or classify it Root or Pure, not \
             {module_class:?}"
        ),
        CrateClass::Effectful { .. } => format!(
            "{file} belongs to the Effectful crate {crate_name}, which may not hold a \
             {module_class:?} module: the composition root lives in the Binary crate"
        ),
        CrateClass::Edge => format!(
            "{file} belongs to the Edge crate {crate_name}, which may not hold a \
             {module_class:?} module: the composition root lives in the Binary crate"
        ),
        CrateClass::Tooling => format!(
            "{file} belongs to the Tooling crate {crate_name}, which holds no product module: \
             remove it from MODULES"
        ),
    }
}

#[test]
fn crate_classes_hold_only_their_module_classes() {
    use ModuleClass::{Command, Root, Shared, Shell, TestSupport};
    let module_classes = [Root, Shell, Command, Shared, ModuleClass::Pure, TestSupport];
    // Each row: a crate class, then whether it holds Root, Shell, Command, Shared, Pure and
    // TestSupport modules.
    let table: [(CrateClass, [bool; 6]); 5] = [
        (
            CrateClass::Pure { feature: "widgets" },
            [false, false, false, false, true, false],
        ),
        (
            CrateClass::Effectful { feature: "widgets" },
            [false, true, true, true, true, true],
        ),
        (CrateClass::Edge, [false, true, true, true, true, true]),
        (CrateClass::Binary, [true, false, false, false, true, false]),
        (
            CrateClass::Tooling,
            [false, false, false, false, false, false],
        ),
    ];
    for (crate_class, expected) in table {
        for (module_class, holds) in module_classes.into_iter().zip(expected) {
            assert_eq!(
                crate_holds_module_class(crate_class, module_class),
                holds,
                "{crate_class:?} holding {module_class:?} ({SECTION})"
            );
        }
    }
}

#[test]
fn module_ids_name_their_crate_directory() {
    let id = |crate_dir: &str, path: &str| {
        Some(ModuleId {
            crate_dir: crate_dir.to_owned(),
            path: path.to_owned(),
        })
    };
    assert_eq!(module_id("src/cli.rs"), id("", "cli"));
    assert_eq!(module_id("src/draw/mod.rs"), id("", "draw"));
    assert_eq!(module_id("src/lib.rs"), id("", "crate"));
    assert_eq!(
        module_id("crates/widgets/core/src/draw/shape.rs"),
        id("crates/widgets/core", "draw::shape")
    );
    assert_eq!(
        module_id("crates/widgets/core/src/draw/mod.rs"),
        id("crates/widgets/core", "draw")
    );
    assert_eq!(
        module_id("crates/widgets/core/src/lib.rs"),
        id("crates/widgets/core", "crate")
    );
    for malformed in [
        "crates/widgets/core/lib.rs",
        "src/cli.txt",
        "src/mod.rs",
        "src//rm.rs",
        "/src/lib.rs",
    ] {
        assert_eq!(module_id(malformed), None, "{malformed}");
    }
}

#[test]
fn at_least_one_pure_module_exists() {
    assert!(
        MODULES.iter().any(|(_, class)| *class == ModuleClass::Pure),
        "no module is classified Pure, so the purity check would scan nothing ({SECTION})"
    );
}
