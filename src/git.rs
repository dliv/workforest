use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

pub fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed in {} (exit code: {})\nstderr: {}",
            args.join(" "),
            repo.display(),
            output
                .status
                .code()
                .map_or("signal".to_string(), |c| c.to_string()),
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(output.stdout).context("git output was not valid UTF-8")?;
    Ok(stdout.trim_end().to_string())
}

pub fn git_stream(repo: &Path, args: &[&str]) -> Result<ExitStatus> {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))?;

    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestEnv;

    #[test]
    fn git_captures_output() {
        let env = TestEnv::new();
        let repo = env.create_repo("test-repo");
        let output = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.len(), 40); // SHA-1 hex
    }

    #[test]
    fn git_returns_error_on_bad_command() {
        let env = TestEnv::new();
        let repo = env.create_repo("test-repo");
        let result = git(&repo, &["log", "--oneline", "--not-a-real-flag"]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("failed"),
            "error should mention failure: {}",
            err
        );
    }

    #[test]
    fn git_stream_returns_exit_status() {
        let env = TestEnv::new();
        let repo = env.create_repo("test-repo");
        let status = git_stream(&repo, &["status"]).unwrap();
        assert!(status.success());
    }

    #[test]
    fn git_stream_returns_failure_status() {
        let env = TestEnv::new();
        let repo = env.create_repo("test-repo");
        let status = git_stream(&repo, &["checkout", "nonexistent-branch"]).unwrap();
        assert!(!status.success());
    }
}
