//! Git's worktree records in listing order, including duplicate paths.
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct WorktreeInventoryEntry {
    pub(crate) path: PathBuf,
    pub(crate) branch_ref: Option<String>,
    pub(crate) lock: WorktreeInventoryLock,
}

#[derive(Debug)]
pub(crate) enum WorktreeInventoryLock {
    Unlocked,
    Locked { reason: Option<String> },
}

pub(crate) fn read_worktree_inventory(source: &Path) -> Result<Vec<WorktreeInventoryEntry>> {
    let output = crate::git::git(source, &["worktree", "list", "--porcelain", "-z"])?;
    let mut entries = Vec::new();
    for record in output.split("\0\0") {
        let mut fields = record.split('\0');
        let Some(path) = fields
            .next()
            .and_then(|field| field.strip_prefix("worktree "))
        else {
            continue;
        };
        let mut branch_ref = None;
        let mut lock = WorktreeInventoryLock::Unlocked;
        for field in fields {
            if let Some(branch) = field.strip_prefix("branch ") {
                branch_ref = Some(branch.to_owned());
            } else if field == "locked" {
                lock = WorktreeInventoryLock::Locked { reason: None };
            } else if let Some(reason) = field.strip_prefix("locked ") {
                lock = WorktreeInventoryLock::Locked {
                    reason: Some(reason.to_owned()),
                };
            }
        }
        entries.push(WorktreeInventoryEntry {
            path: PathBuf::from(path),
            branch_ref,
            lock,
        });
    }
    Ok(entries)
}
