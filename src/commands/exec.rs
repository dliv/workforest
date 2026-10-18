use anyhow::{bail, Result};
use serde::Serialize;
use std::path::Path;

use git_forest_core::meta::ForestMeta;
use git_forest_core::paths::{ForestName, RepoName};

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

/// Runs `cmd` in each repo's worktree, in order. `child_stdout` gives the command's standard
/// output; its standard error is inherited. Progress goes to `on_progress`.
#[expect(
    clippy::disallowed_methods,
    reason = "exec spawns the user's command by design"
)]
pub fn cmd_exec(
    forest_dir: &Path,
    meta: &ForestMeta,
    cmd: &[String],
    child_stdout: &dyn Fn() -> std::process::Stdio,
    on_progress: Option<&dyn Fn(ExecProgress)>,
) -> Result<ExecResult> {
    debug_assert!(forest_dir.is_absolute(), "forest_dir must be absolute");
    if cmd.is_empty() {
        bail!("no command specified\n  hint: put the command after `--`, for example `git forest exec <name> -- make test`; see `git forest exec --help`");
    }

    let mut failures = Vec::new();

    for repo in &meta.repos {
        if let Some(cb) = &on_progress {
            cb(ExecProgress::RepoStarting { name: &repo.name });
        }
        let worktree = forest_dir.join(repo.name.as_str());

        let failure = if !worktree.exists() {
            Some(ExecRepoFailure {
                repo: repo.name.clone(),
                code: ExecFailureCode::WorktreeMissing,
                message: format!("worktree missing at {}", worktree.display()),
                exit_status: None,
            })
        } else {
            let status = std::process::Command::new(&cmd[0])
                .args(&cmd[1..])
                .current_dir(&worktree)
                .stdout(child_stdout())
                .stderr(std::process::Stdio::inherit())
                .status();

            match status {
                Ok(status) if status.success() => None,
                Ok(status) => Some(ExecRepoFailure {
                    repo: repo.name.clone(),
                    code: ExecFailureCode::CommandExited,
                    message: match status.code() {
                        Some(code) => format!("command exited with status {}", code),
                        None => "command terminated by a signal".to_string(),
                    },
                    exit_status: status.code(),
                }),
                Err(e) => Some(ExecRepoFailure {
                    repo: repo.name.clone(),
                    code: ExecFailureCode::SpawnFailed,
                    message: e.to_string(),
                    exit_status: None,
                }),
            }
        };

        if let Some(failure) = failure {
            if let Some(cb) = &on_progress {
                cb(ExecProgress::RepoFailed(&failure));
            }
            failures.push(failure);
        }
    }

    Ok(ExecResult {
        forest_name: meta.name.clone(),
        failures,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::setup_forest_with_git_repos;

    #[test]
    fn cmd_exec_runs_command_in_each_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());

        let cmd = vec!["echo".to_string(), "hello".to_string()];
        let result = cmd_exec(
            &forest_dir,
            &meta,
            &cmd,
            &|| std::process::Stdio::inherit(),
            None,
        )
        .unwrap();
        assert!(result.failures.is_empty());
    }

    #[test]
    fn cmd_exec_empty_cmd_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());

        let result = cmd_exec(
            &forest_dir,
            &meta,
            &[],
            &|| std::process::Stdio::inherit(),
            None,
        );
        assert!(result.is_err());
    }
}
