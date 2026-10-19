//! The result types of `status` and their human renderer. `cmd_status` in the forest ops crate
//! (`git-forest-ops`) reads each worktree's branch state and runs `git status` where that state
//! allows, so `status` has no decide step.

use serde::Serialize;

use crate::paths::{ForestName, RepoName};
use crate::plan::branch_state::{compact_git_error, ActualBranchState, WorktreeBranchState};

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
