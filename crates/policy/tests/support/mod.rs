pub mod classification;
pub mod edges;

/// Root environment items permitted by Workforest's native Clippy inventory.
pub const ROOT_ALLOWED_ENV: &[&str] = &[
    "args_os",
    "ArgsOs",
    "var",
    "var_os",
    "current_dir",
    "current_exe",
];

pub fn ambient_violations(source: &str) -> Vec<String> {
    arch_gates::pure_violations(source)
}

pub fn root_ambient_violations(source: &str) -> Vec<String> {
    arch_gates::scan::root_ambient_violations(source, ROOT_ALLOWED_ENV)
}

pub use arch_gates::graph::crate_edges;
pub use arch_gates::scan::{external_crate_references, trait_declarations};

pub mod metadata {
    pub use arch_gates::metadata::{
        parse_members, Dependency, DependencyKind, Features, ListedMember, Member,
    };
    pub fn workspace_members() -> Result<Vec<Member>, String> {
        let root = git_forest_policy::repo_root();
        parse_members(&git_forest_policy::metadata::cargo_metadata(
            &root.join("Cargo.toml"),
        )?)?
        .into_iter()
        .map(|listed| {
            let text = std::fs::read_to_string(&listed.manifest_path).map_err(|error| {
                format!("cannot read {}: {error}", listed.manifest_path.display())
            })?;
            Ok(listed.with_manifest(&text))
        })
        .collect()
    }
}
