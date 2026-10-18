//! Which class each product module belongs to, and which imports between
//! classes are allowed. The rules are explained in docs/architecture.md,
//! sections "Module classes" and "Import direction".

use std::collections::BTreeSet;

use crate::edges::CrateClass;

/// The architectural role of a source file under a product crate's `src/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleClass {
    /// `src/lib.rs` and the binaries: the composition root. May import anything.
    Root,
    /// A self-contained subsystem with its own effects. Imports Shared and Pure.
    Shell,
    /// A command in `src/commands/`. Imports Shared, Pure and allowlisted Command modules.
    Command,
    /// Types and helpers used across commands. Imports Shared and Pure.
    Shared,
    /// None of the effects the purity scan lists in its own source. Imports only Pure plus
    /// allowlisted edges.
    Pure,
    /// Test helpers compiled only for tests. Not scanned.
    TestSupport,
}

/// Every .rs file under the `src/` directory of every workspace member except Tooling, by
/// repository-relative path, with its class.
pub const MODULES: &[(&str, ModuleClass)] = &[
    ("src/bin/git-forest-beta.rs", ModuleClass::Root),
    ("src/bin/git-forest.rs", ModuleClass::Root),
    ("src/channel.rs", ModuleClass::Pure),
    ("src/cli.rs", ModuleClass::Pure),
    ("src/commands/branch_state.rs", ModuleClass::Command),
    ("src/commands/exec.rs", ModuleClass::Command),
    ("src/commands/init.rs", ModuleClass::Command),
    ("src/commands/ls.rs", ModuleClass::Command),
    ("src/commands/mod.rs", ModuleClass::Command),
    ("src/commands/new.rs", ModuleClass::Command),
    ("src/commands/reset.rs", ModuleClass::Command),
    ("src/commands/rm.rs", ModuleClass::Command),
    ("src/commands/status.rs", ModuleClass::Command),
    ("src/config.rs", ModuleClass::Shared),
    ("src/forest.rs", ModuleClass::Shared),
    ("src/git.rs", ModuleClass::Shared),
    ("src/lib.rs", ModuleClass::Root),
    ("src/meta.rs", ModuleClass::Pure),
    ("src/paths.rs", ModuleClass::Pure),
    ("src/plan/branch_state.rs", ModuleClass::Pure),
    ("src/plan/mod.rs", ModuleClass::Pure),
    ("src/plan/new.rs", ModuleClass::Pure),
    ("src/plan/reset.rs", ModuleClass::Pure),
    ("src/plan/rm.rs", ModuleClass::Pure),
    ("src/testutil.rs", ModuleClass::TestSupport),
    ("src/version_check.rs", ModuleClass::Shell),
];

/// An import edge the class rules would refuse but that is sanctioned, between two modules of
/// one crate.
pub struct AllowedEdge {
    /// The directory of the crate holding both modules, relative to the repository root;
    /// empty for the root package.
    pub crate_dir: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    pub reason: &'static str,
}

const BRANCH_STATE_HELPER: &str =
    "branch_state is the helper shared by the commands that inspect worktree branches";

/// Edges the class rules would refuse but that are sanctioned, each with a reason: the
/// permanent boundary it is, or the change that removes it.
pub const ALLOWED_EDGES: &[AllowedEdge] = &[
    AllowedEdge {
        crate_dir: "",
        from: "commands::ls",
        to: "commands::branch_state",
        reason: BRANCH_STATE_HELPER,
    },
    AllowedEdge {
        crate_dir: "",
        from: "commands::rm",
        to: "commands::branch_state",
        reason: BRANCH_STATE_HELPER,
    },
    AllowedEdge {
        crate_dir: "",
        from: "commands::status",
        to: "commands::branch_state",
        reason: BRANCH_STATE_HELPER,
    },
    AllowedEdge {
        crate_dir: "",
        from: "commands::reset",
        to: "commands::rm",
        reason: "reset observes and executes forest removal through rm's effectful helpers; the removal plan and its decisions live in src/plan/rm.rs, and whether those helpers become a shared module is an open follow-up",
    },
];

/// Dependency crates that Pure code may use, none of them an effect, each with the features a
/// Pure crate may declare for it:
/// - `anyhow` carries errors; no features;
/// - `chrono` supplies date and time types; its clock reads are banned by clippy.toml; `serde`,
///   so that the metadata types serialize their timestamps;
/// - `clap`: src/cli.rs and src/meta.rs derive argument types with clap's derive macros;
///   parsing the arguments, which reads the process's arguments, happens only in src/lib.rs;
///   `derive`, for those macros;
/// - `serde` derives serialization; `derive`, for its derive macros.
///
/// The purity scan reads the names, and every entry must still be used by a Pure module. The
/// edges test reads the features: a Pure crate may declare only the pinned ones. Default
/// features are not checked.
pub const PURE_ALLOWED_CRATES: &[(&str, &[&str])] = &[
    ("anyhow", &[]),
    ("chrono", &["serde"]),
    ("clap", &["derive"]),
    ("serde", &["derive"]),
];

/// A module's identity: the crate it belongs to and its path inside that crate.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId {
    /// The crate's directory, relative to the repository root; empty for the root package.
    pub crate_dir: String,
    /// The module's path inside its crate: `crate` for the crate's `src/lib.rs`.
    pub path: String,
}

/// The identity of a repository-relative `.rs` file, split at its first `src` component:
/// "src/commands/rm.rs" -> ("", "commands::rm"); "src/commands/mod.rs" -> ("", "commands");
/// "src/bin/x.rs" -> ("", "bin::x"); "crates/forest/core/src/plan/rm.rs" ->
/// ("crates/forest/core", "plan::rm"); each crate's "src/lib.rs" is that crate's "crate".
/// None for other paths.
pub fn module_id(relative_file: &str) -> Option<ModuleId> {
    let segments: Vec<&str> = relative_file.split('/').collect();
    let src = segments.iter().position(|segment| *segment == "src")?;
    let (crate_segments, module_segments) = (&segments[..src], &segments[src + 1..]);
    if crate_segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let mut module_segments = module_segments.to_vec();
    let file = module_segments.pop()?.strip_suffix(".rs")?;
    let path = if module_segments.is_empty() && file == "lib" {
        "crate".to_owned()
    } else {
        if file != "mod" {
            module_segments.push(file);
        }
        if module_segments.is_empty() || module_segments.iter().any(|segment| segment.is_empty()) {
            return None;
        }
        module_segments.join("::")
    };
    Some(ModuleId {
        crate_dir: crate_segments.join("/"),
        path,
    })
}

/// The module paths of the crate at `crate_dir` among `modules`: what a path in one of that
/// crate's files can refer to.
pub fn crate_modules(modules: &[(&str, ModuleClass)], crate_dir: &str) -> Vec<String> {
    modules
        .iter()
        .filter_map(|(file, _)| module_id(file))
        .filter(|id| id.crate_dir == crate_dir)
        .map(|id| id.path)
        .collect()
}

/// The crate names Pure modules may use: the names in `PURE_ALLOWED_CRATES`, plus every Pure
/// crate in `crates` as code spells it, hyphens read as underscores.
pub fn crates_allowed_in_pure_modules(crates: &[(&str, CrateClass)]) -> BTreeSet<String> {
    let external = PURE_ALLOWED_CRATES
        .iter()
        .map(|(name, _)| (*name).to_owned());
    let members = crates
        .iter()
        .filter(|(_, class)| matches!(class, CrateClass::Pure { .. }))
        .map(|(name, _)| name.replace('-', "_"));
    external.chain(members).collect()
}

/// Whether class `from` may import class `to` without an allowlist entry.
pub fn class_allows(from: ModuleClass, to: ModuleClass) -> bool {
    match from {
        ModuleClass::Root => true,
        ModuleClass::Shell | ModuleClass::Command | ModuleClass::Shared => {
            matches!(to, ModuleClass::Shared | ModuleClass::Pure)
        }
        ModuleClass::Pure => to == ModuleClass::Pure,
        // Never scanned; it has a class only so that every file under src/ is classified.
        ModuleClass::TestSupport => false,
    }
}
