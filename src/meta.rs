use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

use crate::paths::{
    validate_disposable_root_entries, AbsolutePath, DisposableRootEntry, ForestName, RepoName,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ForestMode {
    Feature,
    Review,
}

impl fmt::Display for ForestMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            ForestMode::Feature => "feature",
            ForestMode::Review => "review",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForestMeta {
    pub name: ForestName,
    pub created_at: DateTime<Utc>,
    pub mode: ForestMode,
    #[serde(default)]
    pub disposable_root_entries: Vec<DisposableRootEntry>,
    pub repos: Vec<RepoMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoMeta {
    pub name: RepoName,
    pub source: AbsolutePath,
    pub branch: String,
    pub base_branch: String,
    #[serde(default)]
    pub remote: Option<String>,
    pub branch_created: bool,
}

impl ForestMeta {
    pub fn write(&self, path: &Path) -> Result<()> {
        self.validate()
            .context("forest metadata has invalid disposable root entries")?;
        let content = toml::to_string_pretty(self).context("failed to serialize forest meta")?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write forest meta to {}", path.display()))?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<ForestMeta> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read forest meta from {}", path.display()))?;
        let meta: ForestMeta =
            toml::from_str(&content).context("failed to parse forest meta TOML")?;
        meta.validate()
            .context("forest metadata has invalid disposable root entries")?;
        Ok(meta)
    }

    fn validate(&self) -> Result<()> {
        validate_disposable_root_entries(
            &self.disposable_root_entries,
            std::iter::once(META_FILENAME).chain(self.repos.iter().map(|repo| repo.name.as_str())),
        )
    }
}

pub const META_FILENAME: &str = ".forest-meta.toml";
pub const STAGED_META_PREFIX: &str = ".git-forest-meta-removing-";

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn sample_meta() -> ForestMeta {
        ForestMeta {
            name: ForestName::new("review-sues-dialog".to_string()).unwrap(),
            created_at: Utc.with_ymd_and_hms(2026, 2, 7, 14, 30, 0).unwrap(),
            mode: ForestMode::Review,
            disposable_root_entries: vec![DisposableRootEntry::new(".idea".to_string()).unwrap()],
            repos: vec![
                RepoMeta {
                    name: RepoName::new("foo-api".to_string()).unwrap(),
                    source: AbsolutePath::new(PathBuf::from("/tmp/src/foo-api")).unwrap(),
                    branch: "forest/review-sues-dialog".to_string(),
                    base_branch: "dev".to_string(),
                    remote: Some("origin".to_string()),
                    branch_created: true,
                },
                RepoMeta {
                    name: RepoName::new("foo-web".to_string()).unwrap(),
                    source: AbsolutePath::new(PathBuf::from("/tmp/src/foo-web")).unwrap(),
                    branch: "sue/gh-100/fix-dialog".to_string(),
                    base_branch: "dev".to_string(),
                    remote: Some("upstream".to_string()),
                    branch_created: false,
                },
            ],
        }
    }

    #[test]
    fn round_trip_write_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(META_FILENAME);

        let original = sample_meta();
        original.write(&path).unwrap();
        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(serialized.contains("disposable_root_entries = [\".idea\"]"));

        let loaded = ForestMeta::read(&path).unwrap();
        assert_eq!(loaded.name, original.name);
        assert_eq!(loaded.created_at, original.created_at);
        assert_eq!(loaded.mode, original.mode);
        assert_eq!(
            loaded.disposable_root_entries,
            original.disposable_root_entries
        );
        assert_eq!(loaded.repos.len(), original.repos.len());
        assert_eq!(loaded.repos[0].name.as_str(), "foo-api");
        assert!(loaded.repos[0].branch_created);
        assert_eq!(loaded.repos[0].remote.as_deref(), Some("origin"));
        assert_eq!(loaded.repos[1].name.as_str(), "foo-web");
        assert_eq!(loaded.repos[1].remote.as_deref(), Some("upstream"));
        assert!(!loaded.repos[1].branch_created);
    }

    #[test]
    fn read_rejects_disposable_entry_that_collides_with_repo() {
        let toml = r#"
name = "test-forest"
created_at = "2026-02-07T14:30:00Z"
mode = "feature"
disposable_root_entries = ["repo-a"]

[[repos]]
name = "repo-a"
source = "/tmp/repo-a"
branch = "user/test-forest"
base_branch = "main"
branch_created = true
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(META_FILENAME);
        std::fs::write(&path, toml).unwrap();

        let error = ForestMeta::read(&path).unwrap_err().to_string();

        assert!(error.contains("invalid disposable root entries"));
    }

    #[test]
    fn read_meta_with_all_fields() {
        let toml = r#"
name = "test-forest"
created_at = "2026-02-07T14:30:00Z"
mode = "feature"

[[repos]]
name = "foo-api"
source = "/tmp/src/foo-api"
branch = "user/test-forest"
base_branch = "dev"
branch_created = true

[[repos]]
name = "dev-docs"
source = "/tmp/src/dev-docs"
branch = "user/test-forest"
base_branch = "main"
branch_created = true
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(META_FILENAME);
        std::fs::write(&path, toml).unwrap();

        let meta = ForestMeta::read(&path).unwrap();
        assert_eq!(meta.name.as_str(), "test-forest");
        assert_eq!(meta.mode, ForestMode::Feature);
        assert_eq!(meta.repos.len(), 2);
        assert_eq!(
            meta.repos[0].source,
            AbsolutePath::new(PathBuf::from("/tmp/src/foo-api")).unwrap()
        );
        assert_eq!(meta.repos[1].base_branch, "main");
        assert_eq!(meta.repos[0].remote, None);
        assert!(meta.disposable_root_entries.is_empty());
    }

    #[test]
    fn handles_review_mode() {
        let toml = r#"
name = "review-pr"
created_at = "2026-02-07T14:30:00Z"
mode = "review"

[[repos]]
name = "foo"
source = "/tmp/foo"
branch = "forest/review-pr"
base_branch = "dev"
branch_created = true
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(META_FILENAME);
        std::fs::write(&path, toml).unwrap();

        let meta = ForestMeta::read(&path).unwrap();
        assert_eq!(meta.mode, ForestMode::Review);
    }
}
