use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Output, Stdio};

#[expect(
    clippy::disallowed_methods,
    reason = "crates/forest/ops/src/git.rs is the one git subprocess wrapper"
)]
fn capture(repo: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))?;

    Ok(output)
}

fn successful_bytes(repo: &Path, args: &[&str], output: Output) -> Result<Vec<u8>> {
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

    Ok(output.stdout)
}

pub fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let stdout =
        String::from_utf8(git_bytes(repo, args)?).context("git output was not valid UTF-8")?;
    Ok(stdout.trim_end().to_string())
}

pub(crate) fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    successful_bytes(repo, args, capture(repo, args)?)
}

/// Presence is distinct from empty values and failed config reads. Callers supply an exact key.
pub(crate) fn config_key_present(repo: &Path, key: &str) -> Result<bool> {
    let args = ["config", "--null", "--get-all", key];
    let output = capture(repo, &args)?;
    if output.status.success() {
        if !output.stdout.ends_with(b"\0") {
            bail!("invalid Git config query output");
        }
        return Ok(true);
    }
    if output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty() {
        return Ok(false);
    }
    successful_bytes(repo, &args, output).map(|_| true)
}

/// Check if a ref exists. Returns true if `git show-ref --verify <refname>` succeeds.
#[expect(
    clippy::disallowed_methods,
    reason = "crates/forest/ops/src/git.rs is the one git subprocess wrapper"
)]
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
        Some(1) => Ok(false),
        Some(code) => {
            if git(repo, &["rev-parse", "--git-dir"]).is_ok() {
                return Ok(false);
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "git show-ref --verify {} failed in {} (exit code: {})\nstderr: {}",
                refname,
                repo.display(),
                code,
                stderr.trim()
            )
        }
        None => bail!(
            "git show-ref --verify {} killed by signal in {}",
            refname,
            repo.display()
        ),
    }
}

/// Whether `path` is inside a git repository, from `git rev-parse --git-dir` run there with its
/// output discarded. `Err` only when git could not be started there.
#[expect(
    clippy::disallowed_methods,
    reason = "crates/forest/ops/src/git.rs is the one git subprocess wrapper"
)]
pub(crate) fn is_git_repository(path: &Path) -> std::io::Result<bool> {
    let status = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
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
    fn config_presence_distinguishes_empty_inherited_missing_and_failed_reads() {
        let env = TestEnv::new();
        let repo = env.create_repo("config-presence");
        let key = "branch.main.merge";
        assert!(!config_key_present(&repo, key).unwrap());
        git(&repo, &["config", key, ""]).unwrap();
        assert!(config_key_present(&repo, key).unwrap());
        git(&repo, &["config", "--unset", key]).unwrap();
        let included = env.root().join("included-config");
        std::fs::write(&included, "[branch \"main\"]\nmerge = refs/heads/main\n").unwrap();
        git(
            &repo,
            &["config", "include.path", included.to_str().unwrap()],
        )
        .unwrap();
        assert!(config_key_present(&repo, key).unwrap());
        std::fs::write(&included, "[invalid\n").unwrap();
        assert!(config_key_present(&repo, key).is_err());
    }

    #[test]
    fn byte_output_preserves_non_utf8_while_string_adapter_refuses_it() {
        let env = TestEnv::new();
        let repo = env.create_repo("bytes");
        std::fs::write(repo.join("raw"), b"\xff\0tail\n").unwrap();
        git(&repo, &["add", "raw"]).unwrap();
        git(&repo, &["commit", "-m", "raw fixture"]).unwrap();
        assert_eq!(
            git_bytes(&repo, &["show", "HEAD:raw"]).unwrap(),
            b"\xff\0tail\n"
        );
        assert!(git(&repo, &["show", "HEAD:raw"]).is_err());
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

    #[test]
    fn ref_exists_errors_for_non_git_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let result = ref_exists(tmp.path(), "refs/heads/main");

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("git show-ref --verify refs/heads/main failed"));
    }

    #[test]
    fn is_git_repository_reports_each_outcome() {
        let env = TestEnv::new();
        let repo = env.create_repo("test-repo");
        assert!(is_git_repository(&repo).unwrap());

        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_git_repository(tmp.path()).unwrap());

        let file = tmp.path().join("not-a-directory");
        std::fs::write(&file, "a regular file").unwrap();
        assert!(is_git_repository(&file).is_err());
    }
}
