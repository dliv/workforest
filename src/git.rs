use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

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

/// Check if a ref exists. Returns true if `git show-ref --verify <refname>` succeeds.
pub fn ref_exists(repo: &Path, refname: &str) -> Result<bool> {
    let output = Command::new("git")
        .args(["show-ref", "--verify", refname])
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| {
            format!(
                "failed to run git show-ref --verify {} in {}",
                refname,
                repo.display()
            )
        })?;

    match output.status.code() {
        Some(0) => Ok(true),
        Some(_) => Ok(false), // exit 1 or 128 both mean "ref not found"
        None => bail!(
            "git show-ref --verify {} killed by signal in {}",
            refname,
            repo.display()
        ),
    }
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
    fn ref_exists_local_branch() {
        let env = TestEnv::new();
        let repo = env.create_repo("test-repo");
        assert!(ref_exists(&repo, "refs/heads/main").unwrap());
    }

    #[test]
    fn ref_exists_local_branch_missing() {
        let env = TestEnv::new();
        let repo = env.create_repo("test-repo");
        assert!(!ref_exists(&repo, "refs/heads/nonexistent").unwrap());
    }

    #[test]
    fn ref_exists_remote_ref() {
        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("test-repo");
        assert!(ref_exists(&repo, "refs/remotes/origin/main").unwrap());
    }

    #[test]
    fn ref_exists_remote_ref_missing() {
        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("test-repo");
        assert!(!ref_exists(&repo, "refs/remotes/origin/nonexistent").unwrap());
    }
}
