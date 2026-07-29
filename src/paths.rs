use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    /// Construct from a PathBuf that is already absolute.
    /// Returns None if not absolute.
    pub fn new(path: PathBuf) -> Option<Self> {
        if path.is_absolute() {
            Some(Self(path))
        } else {
            None
        }
    }

    /// Unwrap to inner PathBuf.
    pub fn into_inner(self) -> PathBuf {
        self.0
    }

    /// Join a relative component, returning a new AbsolutePath.
    /// Safe because absolute + relative = absolute.
    pub fn join<P: AsRef<Path>>(&self, path: P) -> AbsolutePath {
        AbsolutePath(self.0.join(path))
    }
}

impl Deref for AbsolutePath {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for AbsolutePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl std::fmt::Display for AbsolutePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl Serialize for AbsolutePath {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AbsolutePath {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let path = PathBuf::deserialize(deserializer)?;
        AbsolutePath::new(path).ok_or_else(|| serde::de::Error::custom("path must be absolute"))
    }
}

pub fn expand_tilde(path: &str) -> Result<AbsolutePath> {
    let result = if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(rest)
        } else {
            bail!("cannot expand ~/: HOME environment variable is not set");
        }
    } else if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
        } else {
            bail!("cannot expand ~: HOME environment variable is not set");
        }
    } else {
        PathBuf::from(path)
    };

    AbsolutePath::new(result).ok_or_else(|| anyhow::anyhow!("path is not absolute: {}", path))
}

// --- String newtypes ---

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisposableRootEntry(String);

impl DisposableRootEntry {
    pub fn new(entry: String) -> Result<Self> {
        let path = Path::new(&entry);
        let mut components = path.components();
        let is_one_normal_component =
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

        if entry.is_empty()
            || entry.contains('/')
            || entry.contains('\\')
            || !is_one_normal_component
        {
            bail!(
                "invalid disposable root entry: {:?}\n  hint: use one exact name at the forest root, such as .idea",
                entry
            );
        }

        Ok(Self(entry))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl std::fmt::Display for DisposableRootEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for DisposableRootEntry {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::new(value.to_string())
    }
}

impl Serialize for DisposableRootEntry {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DisposableRootEntry {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let entry = String::deserialize(deserializer)?;
        Self::new(entry).map_err(serde::de::Error::custom)
    }
}

pub fn validate_disposable_root_entries<'a>(
    entries: &[DisposableRootEntry],
    reserved_entries: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    // Conservatively fold case because the supported macOS/APFS environment is
    // commonly case-insensitive. Deletion authority must not alias metadata or
    // a managed repository merely through a different spelling.
    let reserved: std::collections::HashSet<String> = reserved_entries
        .into_iter()
        .map(forest_root_entry_comparison_key)
        .collect();
    let mut seen = std::collections::HashSet::new();

    for entry in entries {
        let comparison_key = forest_root_entry_comparison_key(entry.as_str());
        ensure!(
            seen.insert(comparison_key.clone()),
            "duplicate disposable root entry: {}",
            entry
        );
        ensure!(
            !reserved.contains(&comparison_key),
            "disposable root entry {} is reserved\n  hint: choose an incidental root entry that is not forest metadata or a managed repository",
            entry
        );
    }

    Ok(())
}

pub(crate) fn forest_root_entry_comparison_key(entry: &str) -> String {
    entry.to_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoName(String);

impl RepoName {
    pub fn new(name: String) -> Result<Self> {
        if name.is_empty() {
            bail!("repo name must not be empty");
        }
        if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            bail!(
                "invalid repo name: {:?}\n  hint: repo names must be a single directory name, not a path",
                name
            );
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RepoName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for RepoName {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RepoName {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        RepoName::new(s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForestName(String);

impl ForestName {
    pub fn new(name: String) -> Result<Self> {
        if name.is_empty() || name == "." || name == ".." {
            bail!(
                "invalid forest name: {:?}\n  hint: provide a descriptive name like \"java-84/refactor-auth\"",
                name
            );
        }
        let sanitized = sanitize_forest_name(&name);
        if sanitized.is_empty() {
            bail!(
                "forest name {:?} sanitizes to empty\n  hint: provide a name with at least one alphanumeric character",
                name
            );
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The filesystem-safe form (slashes replaced with hyphens).
    pub fn sanitized(&self) -> String {
        sanitize_forest_name(&self.0)
    }
}

impl std::fmt::Display for ForestName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(&self.0)
    }
}

impl Serialize for ForestName {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ForestName {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ForestName::new(s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchName(String);

impl BranchName {
    /// Validate a branch name.
    /// `remote` is needed to reject remote-prefixed names like "origin/main".
    pub fn new(name: String, remote: &str) -> Result<Self> {
        if name.is_empty() {
            bail!("branch name must not be empty");
        }
        if name.starts_with("refs/") {
            bail!(
                "branch name {:?} looks like a ref path\n  hint: pass the branch name without the refs/ prefix",
                name
            );
        }
        let remote_prefix = format!("{}/", remote);
        if name.starts_with(&remote_prefix) {
            bail!(
                "branch name {:?} looks like a remote ref\n  hint: pass the branch name without the remote prefix: {:?}",
                name,
                &name[remote_prefix.len()..]
            );
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for BranchName {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

pub(crate) fn sanitize_forest_name(name: &str) -> String {
    let sanitized = name.replace('/', "-");

    debug_assert!(
        !sanitized.contains('/'),
        "sanitized name must not contain /"
    );

    sanitized
}

pub fn forest_dir(worktree_base: &AbsolutePath, name: &ForestName) -> AbsolutePath {
    worktree_base.join(name.sanitized())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_replaces_home() {
        let home = std::env::var("HOME").unwrap();
        let result = expand_tilde("~/src/foo").unwrap();
        assert_eq!(*result, *PathBuf::from(&home).join("src/foo"));
    }

    #[test]
    fn expand_tilde_bare_tilde() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(*expand_tilde("~").unwrap(), *PathBuf::from(&home));
    }

    #[test]
    fn expand_tilde_leaves_absolute_unchanged() {
        let result = expand_tilde("/usr/local/bin").unwrap();
        assert_eq!(*result, *PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn expand_tilde_relative_path_errors() {
        let result = expand_tilde("foo/bar");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not absolute"));
    }

    #[test]
    fn sanitize_replaces_slashes() {
        assert_eq!(
            sanitize_forest_name("java-84/refactor-auth"),
            "java-84-refactor-auth"
        );
    }

    #[test]
    fn sanitize_no_slashes_unchanged() {
        assert_eq!(sanitize_forest_name("my-feature"), "my-feature");
    }

    #[test]
    fn sanitize_multiple_slashes() {
        assert_eq!(sanitize_forest_name("a/b/c"), "a-b-c");
    }

    #[test]
    fn sanitize_empty() {
        assert_eq!(sanitize_forest_name(""), "");
    }

    #[test]
    fn sanitize_leading_dot() {
        assert_eq!(sanitize_forest_name(".hidden"), ".hidden");
    }

    #[test]
    fn forest_dir_combines_base_and_sanitized_name() {
        let base = AbsolutePath::new(PathBuf::from("/tmp/worktrees")).unwrap();
        let name = ForestName::new("java-84/refactor-auth".to_string()).unwrap();
        assert_eq!(
            *forest_dir(&base, &name),
            *PathBuf::from("/tmp/worktrees/java-84-refactor-auth")
        );
    }

    // --- DisposableRootEntry tests ---

    #[test]
    fn disposable_root_entry_accepts_exact_root_names() {
        for value in [".idea", ".claude", ".DS_Store", "scratch"] {
            let entry = DisposableRootEntry::new(value.to_string()).unwrap();
            assert_eq!(entry.as_str(), value);
            assert_eq!(entry.as_path(), Path::new(value));
        }
    }

    #[test]
    fn disposable_root_entry_rejects_paths_and_special_components() {
        for value in [
            "",
            ".",
            "..",
            "/tmp/cache",
            ".claude/cache",
            r".claude\cache",
        ] {
            assert!(
                DisposableRootEntry::new(value.to_string()).is_err(),
                "disposable root entry should reject {value:?}"
            );
        }
    }

    #[test]
    fn disposable_root_entry_serde_round_trip() {
        let entry = DisposableRootEntry::new(".idea".to_string()).unwrap();
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: DisposableRootEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn disposable_root_entry_set_rejects_duplicates_and_reserved_entries() {
        let duplicate = vec![
            DisposableRootEntry::new(".idea".to_string()).unwrap(),
            DisposableRootEntry::new(".IDEA".to_string()).unwrap(),
        ];
        assert!(validate_disposable_root_entries(&duplicate, [".forest-meta.toml"]).is_err());

        let metadata_alias =
            vec![DisposableRootEntry::new(".FOREST-META.TOML".to_string()).unwrap()];
        assert!(validate_disposable_root_entries(&metadata_alias, [".forest-meta.toml"]).is_err());

        let reserved = vec![DisposableRootEntry::new("REPO-A".to_string()).unwrap()];
        assert!(
            validate_disposable_root_entries(&reserved, [".forest-meta.toml", "repo-a"]).is_err()
        );
    }

    // --- RepoName tests ---

    #[test]
    fn repo_name_new_valid() {
        let name = RepoName::new("foo".to_string()).unwrap();
        assert_eq!(name.as_str(), "foo");
    }

    #[test]
    fn repo_name_new_empty_fails() {
        assert!(RepoName::new("".to_string()).is_err());
    }

    #[test]
    fn repo_name_new_path_component_fails() {
        for name in [".", "..", "../victim", "foo/bar", r"foo\bar"] {
            assert!(
                RepoName::new(name.to_string()).is_err(),
                "repo name should reject path component: {:?}",
                name
            );
        }
    }

    #[test]
    fn repo_name_serde_round_trip() {
        let name = RepoName::new("foo-api".to_string()).unwrap();
        let json = serde_json::to_string(&name).unwrap();
        let deserialized: RepoName = serde_json::from_str(&json).unwrap();
        assert_eq!(name, deserialized);
    }

    #[test]
    fn repo_name_deserialize_empty_fails() {
        let json = r#""""#;
        let result: std::result::Result<RepoName, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn repo_name_deserialize_path_component_fails() {
        let result: std::result::Result<RepoName, _> = serde_json::from_str(r#""../victim""#);
        assert!(result.is_err());
    }

    // --- ForestName tests ---

    #[test]
    fn forest_name_new_valid() {
        let name = ForestName::new("my-feature".to_string()).unwrap();
        assert_eq!(name.as_str(), "my-feature");
    }

    #[test]
    fn forest_name_new_empty_fails() {
        assert!(ForestName::new("".to_string()).is_err());
    }

    #[test]
    fn forest_name_new_dot_fails() {
        assert!(ForestName::new(".".to_string()).is_err());
        assert!(ForestName::new("..".to_string()).is_err());
    }

    #[test]
    fn forest_name_sanitized() {
        let name = ForestName::new("a/b".to_string()).unwrap();
        assert_eq!(name.sanitized(), "a-b");
    }

    #[test]
    fn forest_name_all_slashes_sanitizes_to_non_empty() {
        let name = ForestName::new("////".to_string()).unwrap();
        assert_eq!(name.sanitized(), "----");
    }

    #[test]
    fn forest_name_serde_round_trip() {
        let name = ForestName::new("my-feature".to_string()).unwrap();
        let json = serde_json::to_string(&name).unwrap();
        let deserialized: ForestName = serde_json::from_str(&json).unwrap();
        assert_eq!(name, deserialized);
    }

    #[test]
    fn forest_name_deserialize_empty_fails() {
        let json = r#""""#;
        let result: std::result::Result<ForestName, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // --- BranchName tests ---

    #[test]
    fn branch_name_new_valid() {
        let name = BranchName::new("feature/my-branch".to_string(), "origin").unwrap();
        assert_eq!(name.as_str(), "feature/my-branch");
    }

    #[test]
    fn branch_name_new_refs_prefix_fails() {
        assert!(BranchName::new("refs/heads/main".to_string(), "origin").is_err());
    }

    #[test]
    fn branch_name_new_remote_prefix_fails() {
        assert!(BranchName::new("origin/main".to_string(), "origin").is_err());
    }

    #[test]
    fn branch_name_new_different_remote_ok() {
        // "origin/main" is fine when the remote is "upstream"
        let name = BranchName::new("origin/main".to_string(), "upstream").unwrap();
        assert_eq!(name.as_str(), "origin/main");
    }

    // --- AbsolutePath tests ---

    #[test]
    fn absolute_path_new_absolute() {
        assert!(AbsolutePath::new(PathBuf::from("/foo")).is_some());
    }

    #[test]
    fn absolute_path_new_relative() {
        assert!(AbsolutePath::new(PathBuf::from("foo")).is_none());
    }

    #[test]
    fn absolute_path_join() {
        let p = AbsolutePath::new(PathBuf::from("/foo")).unwrap();
        let joined = p.join("bar");
        assert_eq!(*joined, *PathBuf::from("/foo/bar"));
    }

    #[test]
    fn absolute_path_deref() {
        let p = AbsolutePath::new(PathBuf::from("/foo/bar")).unwrap();
        let path_ref: &Path = &p;
        assert_eq!(path_ref, Path::new("/foo/bar"));
    }

    #[test]
    fn absolute_path_display() {
        let p = AbsolutePath::new(PathBuf::from("/foo/bar")).unwrap();
        assert_eq!(format!("{}", p), "/foo/bar");
    }

    #[test]
    fn absolute_path_serde_round_trip() {
        let p = AbsolutePath::new(PathBuf::from("/foo/bar")).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let deserialized: AbsolutePath = serde_json::from_str(&json).unwrap();
        assert_eq!(p, deserialized);
    }

    #[test]
    fn absolute_path_deserialize_relative_fails() {
        let json = r#""foo/bar""#;
        let result: std::result::Result<AbsolutePath, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn expand_tilde_returns_absolute_path() {
        let result = expand_tilde("/some/absolute/path").unwrap();
        assert!(result.is_absolute());
    }
}
