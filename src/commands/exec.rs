use anyhow::{bail, Result};
use serde::Serialize;
use std::path::Path;

use crate::meta::ForestMeta;

#[derive(Debug, Serialize)]
pub struct ExecResult {
    pub forest_name: String,
    pub failures: Vec<String>,
}

pub fn cmd_exec(forest_dir: &Path, meta: &ForestMeta, cmd: &[String]) -> Result<ExecResult> {
    if cmd.is_empty() {
        bail!("no command specified");
    }

    let mut failures = Vec::new();

    for repo in &meta.repos {
        let worktree = forest_dir.join(&repo.name);
        eprintln!("=== {} ===", repo.name);

        if !worktree.exists() {
            eprintln!("  warning: worktree missing at {}", worktree.display());
            failures.push(repo.name.clone());
            continue;
        }

        let status = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .current_dir(&worktree)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status();

        match status {
            Ok(s) if !s.success() => {
                failures.push(repo.name.clone());
            }
            Err(e) => {
                eprintln!("  error: {}", e);
                failures.push(repo.name.clone());
            }
            _ => {}
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
        format!("\nFailed in: {}", result.failures.join(", "))
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
        let result = cmd_exec(&forest_dir, &meta, &cmd).unwrap();
        assert!(result.failures.is_empty());
    }

    #[test]
    fn cmd_exec_empty_cmd_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());

        let result = cmd_exec(&forest_dir, &meta, &[]);
        assert!(result.is_err());
    }
}
