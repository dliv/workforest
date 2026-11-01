pub mod classification;
pub mod edges;
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
