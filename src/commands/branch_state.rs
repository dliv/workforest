//! Reading a worktree's branch state from git. The types and their pure functions are in
//! `crate::plan::branch_state`.

use std::path::{Path, PathBuf};

pub(crate) use crate::plan::branch_state::compact_git_error;
pub use crate::plan::branch_state::{ActualBranchState, WorktreeBranchState};

/// Reads the branch state of the worktree at `worktree`, expected on `expected_branch`.
pub fn read_branch_state(worktree: &Path, expected_branch: &str) -> WorktreeBranchState {
    let actual = if path_exists_or_symlink(worktree) {
        read_actual_branch(worktree)
    } else {
        ActualBranchState::MissingWorktree
    };
    WorktreeBranchState::new(expected_branch.to_string(), actual)
}

pub(crate) fn path_exists_or_symlink(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
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
