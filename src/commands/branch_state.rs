use serde::Serialize;
use std::path::{Path, PathBuf};

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
    pub fn read(worktree: &Path, expected_branch: &str) -> Self {
        let actual = if path_exists_or_symlink(worktree) {
            read_actual_branch(worktree)
        } else {
            ActualBranchState::MissingWorktree
        };
        Self::new(expected_branch.to_string(), actual)
    }

    #[cfg(test)]
    pub fn missing_worktree(expected_branch: &str) -> Self {
        Self::new(
            expected_branch.to_string(),
            ActualBranchState::MissingWorktree,
        )
    }

    fn new(expected_branch: String, actual: ActualBranchState) -> Self {
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

pub(crate) fn path_exists_or_symlink(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
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

fn read_actual_branch(worktree: &Path) -> ActualBranchState {
    if let Err(branch_lookup_error) = verify_worktree_root(worktree) {
        return ActualBranchState::Unknown {
            branch_lookup_error,
        };
    }

    if let Ok(actual_branch) =
        crate::git::git(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"])
    {
        return ActualBranchState::Branch { actual_branch };
    }

    match crate::git::git(worktree, &["rev-parse", "HEAD"]) {
        Ok(actual_detached_head) => ActualBranchState::Detached {
            actual_detached_head,
        },
        Err(e) => ActualBranchState::Unknown {
            branch_lookup_error: e.to_string(),
        },
    }
}

fn verify_worktree_root(worktree: &Path) -> Result<(), String> {
    let top_level =
        crate::git::git(worktree, &["rev-parse", "--show-toplevel"]).map_err(|e| e.to_string())?;
    let top_level = PathBuf::from(top_level);
    let canonical_top_level = canonicalize_for_compare(&top_level)?;
    let canonical_worktree = canonicalize_for_compare(worktree)?;

    if canonical_top_level != canonical_worktree {
        return Err(format!(
            "not a git worktree root: {} belongs to {}",
            worktree.display(),
            top_level.display()
        ));
    }

    Ok(())
}

fn canonicalize_for_compare(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|e| format!("failed to canonicalize {}: {}", path.display(), e))
}

fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}
