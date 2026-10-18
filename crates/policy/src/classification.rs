//! Which class each product module belongs to, and which imports between
//! classes are allowed. The rules are explained in docs/architecture.md,
//! sections "Module classes" and "Import direction".

/// The architectural role of a source file under `src/`.
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

/// Every .rs file under src/, by repository-relative path, with its class.
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

/// An import edge the class rules would refuse but that is sanctioned.
pub struct AllowedEdge {
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
        from: "commands::ls",
        to: "commands::branch_state",
        reason: BRANCH_STATE_HELPER,
    },
    AllowedEdge {
        from: "commands::status",
        to: "commands::branch_state",
        reason: BRANCH_STATE_HELPER,
    },
    AllowedEdge {
        from: "commands::reset",
        to: "commands::rm",
        reason: "reset observes and executes forest removal through rm's effectful helpers; the removal plan and its decisions live in src/plan/rm.rs, and whether those helpers become a shared module is an open follow-up",
    },
];

/// Dependency crates of the root package that Pure modules may use, none of them an effect:
/// - `anyhow` carries errors;
/// - `chrono` supplies date and time types; its clock reads are banned by clippy.toml;
/// - `clap`: src/cli.rs and src/meta.rs derive argument types with clap's derive macros;
///   parsing the arguments, which reads the process's arguments, happens only in src/lib.rs;
/// - `serde` derives serialization.
///
/// Every entry must still be used by a Pure module.
pub const PURE_ALLOWED_CRATES: &[&str] = &["anyhow", "chrono", "clap", "serde"];

/// "src/lib.rs" -> "crate"; "src/meta.rs" -> "meta"; "src/commands/mod.rs" -> "commands";
/// "src/commands/rm.rs" -> "commands::rm"; "src/bin/x.rs" -> "bin::x". None for other paths.
pub fn module_path(relative_file: &str) -> Option<String> {
    let path = relative_file.strip_prefix("src/")?.strip_suffix(".rs")?;
    if path == "lib" {
        return Some("crate".to_owned());
    }
    let mut segments: Vec<&str> = path.split('/').collect();
    if segments.last() == Some(&"mod") {
        segments.pop();
    }
    if segments.is_empty() || segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    Some(segments.join("::"))
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
