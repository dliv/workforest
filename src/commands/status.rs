use anyhow::Result;
use serde::Serialize;
use std::path::Path;

use super::branch_state::{
    compact_git_error, path_exists_or_symlink, ActualBranchState, WorktreeBranchState,
};
use crate::meta::ForestMeta;
use crate::paths::{ForestName, RepoName};

#[derive(Debug, Serialize)]
pub struct StatusResult {
    pub forest_name: ForestName,
    pub repos: Vec<RepoStatus>,
}

#[derive(Debug, Serialize)]
pub struct RepoStatus {
    pub name: RepoName,
    pub branch_state: WorktreeBranchState,
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
        let worktree = forest_dir.join(repo.name.as_str());
        let branch_state = WorktreeBranchState::read(&worktree, &repo.branch);

        let status = if !path_exists_or_symlink(&worktree) {
            RepoStatusKind::Missing {
                path: worktree.display().to_string(),
            }
        } else if let ActualBranchState::Unknown {
            branch_lookup_error,
        } = &branch_state.actual
        {
            RepoStatusKind::Error {
                message: branch_lookup_error.clone(),
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
            branch_state,
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
        if let Some(message) = repo.branch_state.drift_message() {
            lines.push(format!("  warning: {}", message));
        } else if let Some(message) = repo.branch_state.lookup_error_message() {
            lines.push(format!("  warning: {}", message));
        }
        match &repo.status {
            RepoStatusKind::Ok { output } => lines.push(output.clone()),
            RepoStatusKind::Missing { path } => {
                lines.push(format!("  warning: worktree missing at {}", path));
            }
            RepoStatusKind::Error { message } => {
                if !branch_lookup_error_matches(&repo.branch_state.actual, message) {
                    lines.push(format!("  warning: {}", compact_git_error(message)));
                }
            }
        }
    }
    lines.join("\n")
}

fn branch_lookup_error_matches(actual: &ActualBranchState, message: &str) -> bool {
    matches!(
        actual,
        ActualBranchState::Unknown {
            branch_lookup_error
        } if branch_lookup_error == message
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{cmd_new, NewInputs};
    use crate::meta::ForestMode;
    use crate::testutil::{make_meta, make_repo, setup_forest_with_git_repos, TestEnv};
    use chrono::Utc;
    use std::path::PathBuf;

    fn make_new_inputs(name: &str) -> NewInputs {
        NewInputs {
            name: name.to_string(),
            mode: ForestMode::Feature,
            branch_override: None,
            repo_branches: vec![],
            no_fetch: true,
            dry_run: false,
        }
    }

    fn setup_single_repo_forest(name: &str) -> (TestEnv, PathBuf, ForestMeta) {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        cmd_new(make_new_inputs(name), &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join(name);
        let meta = ForestMeta::read(&forest_dir.join(crate::meta::META_FILENAME)).unwrap();
        (env, forest_dir.to_path_buf(), meta)
    }

    fn switch_forest_worktree_to_main(forest_dir: &Path, meta: &ForestMeta) {
        let source = &meta.repos[0].source;
        crate::git::git(source, &["checkout", "-b", "source-other"]).unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["checkout", "main"]).unwrap();
    }

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
        assert_eq!(result.repos[0].branch_state.expected_branch, "main");
        assert!(!result.repos[0].branch_state.branch_drift);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["repos"][0]["branch_state"]["actual_type"],
            "missing_worktree"
        );
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], false);
    }

    #[test]
    fn cmd_status_reports_branch_metadata_drift() {
        let (_env, forest_dir, meta) = setup_single_repo_forest("status-drift");
        switch_forest_worktree_to_main(&forest_dir, &meta);

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];
        assert_eq!(repo.branch_state.expected_branch, "testuser/status-drift");
        assert!(repo.branch_state.branch_drift);
        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Branch { actual_branch } if actual_branch == "main"
        ));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["repos"][0]["branch_state"]["expected_branch"],
            "testuser/status-drift"
        );
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "branch");
        assert_eq!(json["repos"][0]["branch_state"]["actual_branch"], "main");
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], true);

        let human = format_status_human(&result);
        assert!(human.contains("branch drift"));
        assert!(human.contains("expected testuser/status-drift"));
        assert!(human.contains("actual main"));
    }

    #[test]
    fn cmd_status_reports_detached_head_drift() {
        let (_env, forest_dir, meta) = setup_single_repo_forest("status-detached");
        let worktree = forest_dir.join("foo-api");
        let head = crate::git::git(&worktree, &["rev-parse", "HEAD"]).unwrap();
        crate::git::git(&worktree, &["checkout", "--detach", "HEAD"]).unwrap();

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];
        assert_eq!(
            repo.branch_state.expected_branch,
            "testuser/status-detached"
        );
        assert!(repo.branch_state.branch_drift);
        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Detached {
                actual_detached_head
            } if actual_detached_head == &head
        ));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "detached");
        assert_eq!(
            json["repos"][0]["branch_state"]["actual_detached_head"],
            head
        );
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], true);

        let human = format_status_human(&result);
        assert!(human.contains("branch drift"));
        assert!(human.contains("detached HEAD"));
    }

    #[test]
    fn cmd_status_reports_branch_lookup_failure_for_present_non_git_dir() {
        let (_env, forest_dir, meta) = setup_single_repo_forest("status-lookup-error");
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join("README.txt"), "not a git worktree").unwrap();

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];

        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(repo.status, RepoStatusKind::Error { .. }));

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "unknown");
        assert!(json["repos"][0]["branch_state"]["branch_lookup_error"]
            .as_str()
            .unwrap()
            .contains("git rev-parse --show-toplevel failed"));

        let human = format_status_human(&result);
        assert!(human.contains("branch lookup failed"));
        assert!(human.contains("git rev-parse --show-toplevel failed"));
        assert_eq!(
            human
                .matches("git rev-parse --show-toplevel failed")
                .count(),
            1,
            "status should not duplicate branch lookup failures:\n{}",
            human
        );
        assert!(human.contains("stderr: fatal:"));
        assert!(
            !human.lines().any(|line| line.starts_with("stderr:")),
            "status human output should keep git diagnostics indented and single-line: {}",
            human
        );
    }

    #[cfg(unix)]
    #[test]
    fn cmd_status_reports_dangling_symlink_as_lookup_error() {
        let (_env, forest_dir, meta) = setup_single_repo_forest("status-dangling-symlink");
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink("/missing/target", &worktree).unwrap();

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];

        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(&repo.status, RepoStatusKind::Error { .. }));

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "unknown");
        assert_eq!(json["repos"][0]["status"]["type"], "Error");

        let human = format_status_human(&result);
        assert!(human.contains("branch lookup failed"));
        assert!(!human.contains("worktree missing"));
    }

    #[test]
    fn cmd_status_treats_plain_dir_inside_parent_repo_as_lookup_error() {
        let env = TestEnv::new();
        let parent = env.create_repo("parent");
        let forest_dir = parent.join("lookup-parent");
        let repo_dir = forest_dir.join("not-git");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::write(repo_dir.join("README.txt"), "not a git worktree").unwrap();

        let meta = make_meta(
            "lookup-parent",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("not-git", "dliv/lookup-parent")],
        );

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];

        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Unknown {
                branch_lookup_error
            } if branch_lookup_error.contains("not a git worktree root")
        ));
        assert!(matches!(
            &repo.status,
            RepoStatusKind::Error { message } if message.contains("not a git worktree root")
        ));

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "unknown");
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], false);
        assert!(json["repos"][0]["branch_state"]["branch_lookup_error"]
            .as_str()
            .unwrap()
            .contains("not a git worktree root"));

        let human = format_status_human(&result);
        assert!(human.contains("branch lookup failed"));
        assert!(human.contains("not a git worktree root"));
        assert!(!human.contains("## main"));
    }
}
