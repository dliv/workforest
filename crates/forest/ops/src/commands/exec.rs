use anyhow::{bail, Result};
use std::path::Path;

use git_forest_core::meta::ForestMeta;
use git_forest_core::plan::exec::ExecRepoFailure;
pub use git_forest_core::plan::exec::{
    format_exec_human, ExecFailureCode, ExecProgress, ExecResult,
};

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
