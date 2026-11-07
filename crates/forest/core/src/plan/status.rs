//! The result types of `status` and their human renderer. `cmd_status` in the forest ops crate
//! (`git-forest-ops`) reads each worktree's branch state and runs `git status` where that state
//! allows, so `status` has no decide step.

use crate::plan::lock_state::WorktreeLockState;
use crate::plan::status_protocol::CommitId;
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
    pub lock_state: WorktreeLockState,
    pub branch_state: WorktreeBranchState,
    pub status: RepoStatusKind,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum RepoStatusKind {
    Ok {
        output: String,
        summary: StatusSummary,
    },
    Missing {
        path: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatusSummary {
    Available(Box<AvailableStatusSummary>),
    Unavailable {
        reason: SummaryUnavailableReason,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AvailableStatusSummary {
    pub head_commit: Option<CommitId>,
    pub changed_path_count: u64,
    pub upstream: UpstreamComparison,
    pub base: BaseComparison,
    pub commits_not_on_known_remotes: KnownRemoteCommits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryUnavailableReason {
    ObservationFailed,
    InvalidOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitComparison {
    pub r#ref: String,
    pub ahead: u64,
    pub behind: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpstreamComparison {
    Available(CommitComparison),
    NotConfigured,
    Detached,
    Unavailable {
        r#ref: Option<String>,
        reason: ComparisonUnavailableReason,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonUnavailableReason {
    TargetUnavailable,
    ObservationFailed,
    ObservationChanged,
    UnbornHead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BaseComparison {
    Available(CommitComparison),
    Unavailable {
        r#ref: Option<String>,
        reason: BaseUnavailableReason,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseUnavailableReason {
    TargetUnavailable,
    ObservationFailed,
    UnbornHead,
    MetadataRemoteMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownRemoteCommits {
    Available {
        count: u64,
    },
    Unavailable {
        reason: RemoteCountUnavailableReason,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCountUnavailableReason {
    NoRemoteRefs,
    UnbornHead,
    ObservationFailed,
    InvalidOutput,
}

pub fn format_status_human(result: &StatusResult) -> String {
    let mut lines = Vec::new();
    for repo in &result.repos {
        lines.push(format!("=== {} ===", repo.name));
        if let Some(annotation) = repo.lock_state.human_annotation() {
            lines.push(format!("  {}", annotation));
        }
        if let Some(message) = repo.branch_state.drift_message() {
            lines.push(format!("  warning: {}", message));
        } else if let Some(message) = repo.branch_state.lookup_error_message() {
            lines.push(format!("  warning: {}", message));
        }
        match &repo.status {
            RepoStatusKind::Ok { output, .. } => lines.push(output.clone()),
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
