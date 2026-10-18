//! The branch state of a forest's worktree: which branch it was created for and what it has
//! checked out now. Pure: reading the state from git is `WorktreeBranchState::read` in
//! `commands::branch_state`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorktreeBranchState {
    pub expected_branch: String,
    #[serde(flatten)]
    pub actual: ActualBranchState,
    pub branch_drift: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "actual_type")]
pub enum ActualBranchState {
    Branch { actual_branch: String },
    Detached { actual_detached_head: String },
    MissingWorktree,
    Unknown { branch_lookup_error: String },
}

impl WorktreeBranchState {
    #[cfg(test)]
    pub fn missing_worktree(expected_branch: &str) -> Self {
        Self::new(
            expected_branch.to_string(),
            ActualBranchState::MissingWorktree,
        )
    }

    pub(crate) fn new(expected_branch: String, actual: ActualBranchState) -> Self {
        let branch_drift = match &actual {
            ActualBranchState::Branch { actual_branch } => actual_branch != &expected_branch,
            ActualBranchState::Detached { .. } => true,
            ActualBranchState::MissingWorktree | ActualBranchState::Unknown { .. } => false,
        };

        Self {
            expected_branch,
            actual,
            branch_drift,
        }
    }

    pub fn drift_message(&self) -> Option<String> {
        self.branch_drift.then(|| {
            format!(
                "branch drift: expected {}, actual {}",
                self.expected_branch,
                self.actual_display()
            )
        })
    }

    pub fn lookup_error_message(&self) -> Option<String> {
        match &self.actual {
            ActualBranchState::Unknown {
                branch_lookup_error,
            } => Some(format!(
                "branch lookup failed: {}",
                compact_git_error(branch_lookup_error)
            )),
            _ => None,
        }
    }

    fn actual_display(&self) -> String {
        match &self.actual {
            ActualBranchState::Branch { actual_branch } => actual_branch.clone(),
            ActualBranchState::Detached {
                actual_detached_head,
            } => format!("detached HEAD {}", short_commit(actual_detached_head)),
            ActualBranchState::MissingWorktree => "missing worktree".to_string(),
            ActualBranchState::Unknown {
                branch_lookup_error,
            } => format!("unknown ({})", branch_lookup_error),
        }
    }
}

pub(crate) fn compact_git_error(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message);
    let stderr = message.lines().find_map(|line| {
        line.strip_prefix("stderr:")
            .map(str::trim)
            .filter(|line| !line.is_empty())
    });

    match stderr {
        Some(stderr) => format!("{}; stderr: {}", first, stderr),
        None => first.to_string(),
    }
}

fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}
