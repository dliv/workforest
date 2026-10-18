use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

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
    /// Checks the disposable root entries against the metadata file and the repo names.
    pub(crate) fn validate(&self) -> Result<()> {
        validate_disposable_root_entries(
            &self.disposable_root_entries,
            std::iter::once(META_FILENAME).chain(self.repos.iter().map(|repo| repo.name.as_str())),
        )
    }
}

pub const META_FILENAME: &str = ".forest-meta.toml";
pub const STAGED_META_PREFIX: &str = ".git-forest-meta-removing-";
