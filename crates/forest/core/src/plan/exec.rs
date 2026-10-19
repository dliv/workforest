//! The result, failure and progress types of `exec` and its human renderer. `cmd_exec` in the
//! forest ops crate (`git-forest-ops`) runs the user's command repo by repo and reports progress
//! as it goes, so `exec` has no decide step.

use serde::Serialize;

use crate::paths::{ForestName, RepoName};

#[derive(Debug, Serialize)]
pub struct ExecResult {
    pub forest_name: ForestName,
    pub failures: Vec<ExecRepoFailure>,
}

/// One repo where the command did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecRepoFailure {
    pub repo: RepoName,
    pub code: ExecFailureCode,
    pub message: String,
    /// The command's exit status, when it exited with one.
    pub exit_status: Option<i32>,
}

/// Why the command did not succeed in a repo. Serialized kebab-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecFailureCode {
    /// The repo's worktree is missing, so the command did not run there.
    WorktreeMissing,
    /// The command ran and exited unsuccessfully, or was terminated by a signal.
    CommandExited,
    /// The command could not be started.
    SpawnFailed,
}

/// Progress of `exec`, reported to the caller instead of printed.
pub enum ExecProgress<'a> {
    RepoStarting { name: &'a RepoName },
    RepoFailed(&'a ExecRepoFailure),
}

pub fn format_exec_human(result: &ExecResult) -> String {
    if result.failures.is_empty() {
        String::new()
    } else {
        let repos: Vec<String> = result
            .failures
            .iter()
            .map(|failure| failure.repo.to_string())
            .collect();
        format!("\nFailed in: {}", repos.join(", "))
    }
}
