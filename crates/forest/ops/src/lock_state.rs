//! Read-only lock observation, independent of branch and directory health.
use git_forest_core::plan::lock_state::WorktreeLockState;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) fn read_lock_state(source: &Path, worktree: &Path) -> WorktreeLockState {
    read_lock_inventory(source).state_for(worktree)
}

/// One observation of a source repository. Retain failures too, so a caller can
/// reuse the same observation for every forest that refers to this source.
pub(crate) enum WorktreeLockInventory {
    Available(HashMap<PathBuf, WorktreeLockState>),
    Unavailable,
}

impl WorktreeLockInventory {
    pub(crate) fn state_for(&self, worktree: &Path) -> WorktreeLockState {
        match self {
            Self::Available(states) => states
                .get(&path_key(worktree))
                .cloned()
                .unwrap_or(WorktreeLockState::Unavailable),
            Self::Unavailable => WorktreeLockState::Unavailable,
        }
    }
}

pub(crate) fn read_lock_inventory(source: &Path) -> WorktreeLockInventory {
    let Ok(output) = crate::git::git(source, &["worktree", "list", "--porcelain", "-z"]) else {
        return WorktreeLockInventory::Unavailable;
    };
    let mut states = HashMap::new();
    for record in output.split("\0\0") {
        let mut fields = record.split('\0');
        let Some(path) = fields
            .next()
            .and_then(|field| field.strip_prefix("worktree "))
        else {
            continue;
        };
        let state = fields
            .find_map(|field| {
                if field == "locked" {
                    Some(WorktreeLockState::Locked { reason: None })
                } else {
                    field
                        .strip_prefix("locked ")
                        .map(|reason| WorktreeLockState::Locked {
                            reason: Some(reason.to_string()),
                        })
                }
            })
            .unwrap_or(WorktreeLockState::Unlocked);
        states.insert(path_key(Path::new(path)), state);
    }
    WorktreeLockInventory::Available(states)
}

// Resolve aliases in the containing directory even when the worktree itself is missing.
// Do not follow a replacement symlink into another worktree's registration.
fn path_key(path: &Path) -> PathBuf {
    path.parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(path.file_name())
        .map(|(parent, leaf)| parent.join(leaf))
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestEnv;

    #[test]
    fn inventory_matches_expected_path_independently_of_branch() {
        let env = TestEnv::new();
        let source = env.create_repo("source");
        let worktree = source.parent().unwrap().join("worktree with\nnewline");
        crate::git::git(
            &source,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        )
        .unwrap();
        assert_eq!(
            read_lock_state(&source, &worktree),
            WorktreeLockState::Unlocked
        );
        crate::git::git(
            &source,
            &[
                "worktree",
                "lock",
                "--reason",
                "stop dev first",
                worktree.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(
            read_lock_state(&source, &worktree),
            WorktreeLockState::Locked {
                reason: Some("stop dev first".to_string())
            }
        );
        // A lock in another inventory record must not be attributed to the target.
        assert_eq!(
            read_lock_state(&source, &source),
            WorktreeLockState::Unlocked
        );
        assert_eq!(
            read_lock_state(&source, &source.join("unregistered")),
            WorktreeLockState::Unavailable
        );
    }

    #[cfg(unix)]
    #[test]
    fn inventory_matches_missing_worktree_through_aliased_parent() {
        let env = TestEnv::new();
        let source = env.create_repo("source");
        let parent = source.parent().unwrap();
        let worktree = parent.join("linked");
        crate::git::git(
            &source,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        )
        .unwrap();
        crate::git::git(&source, &["worktree", "lock", worktree.to_str().unwrap()]).unwrap();
        let alias = parent.join("alias");
        std::os::unix::fs::symlink(parent, &alias).unwrap();
        std::fs::remove_dir_all(&worktree).unwrap();
        assert_eq!(
            read_lock_state(&source, &alias.join("linked")),
            WorktreeLockState::Locked { reason: None }
        );
        // Replacement symlinks do not borrow the registration of their target.
        std::os::unix::fs::symlink(&source, parent.join("replacement")).unwrap();
        assert_eq!(
            read_lock_state(&source, &parent.join("replacement")),
            WorktreeLockState::Unavailable
        );
    }
}
