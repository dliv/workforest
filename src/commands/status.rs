use anyhow::Result;
use serde::Serialize;
use std::path::Path;

use crate::meta::ForestMeta;

#[derive(Debug, Serialize)]
pub struct StatusResult {
    pub forest_name: String,
    pub repos: Vec<RepoStatus>,
}

#[derive(Debug, Serialize)]
pub struct RepoStatus {
    pub name: String,
    pub status: RepoStatusKind,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum RepoStatusKind {
    Ok { output: String },
    Missing { path: String },
    Error { message: String },
}

pub fn cmd_status(forest_dir: &Path, meta: &ForestMeta) -> Result<StatusResult> {
    debug_assert!(forest_dir.is_absolute(), "forest_dir must be absolute");
    let mut repos = Vec::new();

    for repo in &meta.repos {
        let worktree = forest_dir.join(&repo.name);

        let status = if !worktree.exists() {
            RepoStatusKind::Missing {
                path: worktree.display().to_string(),
            }
        } else {
            match crate::git::git(&worktree, &["status", "-sb"]) {
                Ok(output) => RepoStatusKind::Ok { output },
                Err(e) => RepoStatusKind::Error {
                    message: e.to_string(),
                },
            }
        };

        repos.push(RepoStatus {
            name: repo.name.clone(),
            status,
        });
    }

    Ok(StatusResult {
        forest_name: meta.name.clone(),
        repos,
    })
}

pub fn format_status_human(result: &StatusResult) -> String {
    let mut lines = Vec::new();
    for repo in &result.repos {
        lines.push(format!("=== {} ===", repo.name));
        match &repo.status {
            RepoStatusKind::Ok { output } => lines.push(output.clone()),
            RepoStatusKind::Missing { path } => {
                lines.push(format!("  warning: worktree missing at {}", path));
            }
            RepoStatusKind::Error { message } => {
                lines.push(format!("  warning: {}", message));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::ForestMode;
    use crate::testutil::{make_meta, make_repo, setup_forest_with_git_repos};
    use chrono::Utc;

    #[test]
    fn cmd_status_runs_in_each_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());
        let result = cmd_status(&forest_dir, &meta).unwrap();
        assert_eq!(result.repos.len(), 2);
        assert!(matches!(result.repos[0].status, RepoStatusKind::Ok { .. }));
        assert!(matches!(result.repos[1].status, RepoStatusKind::Ok { .. }));
    }

    #[test]
    fn cmd_status_missing_worktree_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("test-forest");
        std::fs::create_dir_all(&forest_dir).unwrap();

        let meta = make_meta(
            "test-forest",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("missing-repo", "main")],
        );

        let result = cmd_status(&forest_dir, &meta).unwrap();
        assert_eq!(result.repos.len(), 1);
        assert!(matches!(
            result.repos[0].status,
            RepoStatusKind::Missing { .. }
        ));
    }
}
