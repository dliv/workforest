use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ForestMode {
    Feature,
    Review,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForestMeta {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub mode: ForestMode,
    pub repos: Vec<RepoMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoMeta {
    pub name: String,
    pub source: PathBuf,
    pub branch: String,
    pub base_branch: String,
    pub branch_created: bool,
}

impl ForestMeta {
    pub fn write(&self, path: &Path) -> Result<()> {
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
        Ok(meta)
    }
}

pub const META_FILENAME: &str = ".forest-meta.toml";

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_meta() -> ForestMeta {
        ForestMeta {
            name: "review-sues-dialog".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 2, 7, 14, 30, 0).unwrap(),
            mode: ForestMode::Review,
            repos: vec![
                RepoMeta {
                    name: "foo-api".to_string(),
                    source: PathBuf::from("/Users/dliv/src/foo-api"),
                    branch: "forest/review-sues-dialog".to_string(),
                    base_branch: "dev".to_string(),
                    branch_created: true,
                },
                RepoMeta {
                    name: "foo-web".to_string(),
                    source: PathBuf::from("/Users/dliv/src/foo-web"),
                    branch: "sue/gh-100/fix-dialog".to_string(),
                    base_branch: "dev".to_string(),
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

        let loaded = ForestMeta::read(&path).unwrap();
        assert_eq!(loaded.name, original.name);
        assert_eq!(loaded.created_at, original.created_at);
        assert_eq!(loaded.mode, original.mode);
        assert_eq!(loaded.repos.len(), original.repos.len());
        assert_eq!(loaded.repos[0].name, "foo-api");
        assert_eq!(loaded.repos[0].branch_created, true);
        assert_eq!(loaded.repos[1].name, "foo-web");
        assert_eq!(loaded.repos[1].branch_created, false);
    }

    #[test]
    fn read_meta_with_all_fields() {
        let toml = r#"
name = "test-forest"
created_at = "2026-02-07T14:30:00Z"
mode = "feature"

[[repos]]
name = "foo-api"
source = "/Users/dliv/src/foo-api"
branch = "dliv/test-forest"
base_branch = "dev"
branch_created = true

[[repos]]
name = "dev-docs"
source = "/Users/dliv/src/dev-docs"
branch = "dliv/test-forest"
base_branch = "main"
branch_created = true
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(META_FILENAME);
        std::fs::write(&path, toml).unwrap();

        let meta = ForestMeta::read(&path).unwrap();
        assert_eq!(meta.name, "test-forest");
        assert_eq!(meta.mode, ForestMode::Feature);
        assert_eq!(meta.repos.len(), 2);
        assert_eq!(meta.repos[0].source, PathBuf::from("/Users/dliv/src/foo-api"));
        assert_eq!(meta.repos[1].base_branch, "main");
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
