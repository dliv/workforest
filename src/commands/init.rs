use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::{ResolvedConfig, ResolvedRepo};
use crate::paths::expand_tilde;

pub struct InitInputs {
    pub worktree_base: String,
    pub base_branch: String,
    pub branch_template: String,
    pub username: String,
    pub repos: Vec<RepoInput>,
}

pub struct RepoInput {
    pub path: String,
    pub name: Option<String>,
    pub base_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InitResult {
    pub config_path: PathBuf,
    pub worktree_base: PathBuf,
    pub repos: Vec<InitRepoSummary>,
}

#[derive(Debug, Serialize)]
pub struct InitRepoSummary {
    pub name: String,
    pub path: PathBuf,
    pub base_branch: String,
}

pub fn validate_init_inputs(inputs: &InitInputs) -> Result<ResolvedConfig> {
    if inputs.username.is_empty() {
        bail!("--username is required\nHint: git forest init --username <your-name> --repo <path>");
    }

    if inputs.repos.is_empty() {
        bail!("at least one --repo is required\nHint: git forest init --username <your-name> --repo <path>");
    }

    if !inputs.branch_template.contains("{name}") {
        bail!("--branch-template must contain {{name}}");
    }

    let worktree_base = expand_tilde(&inputs.worktree_base);

    let mut resolved_repos = Vec::new();
    let mut names = HashSet::new();

    for repo_input in &inputs.repos {
        let path = expand_tilde(&repo_input.path);

        if !path.exists() {
            bail!(
                "repo path does not exist: {}\nHint: provide an absolute path to a git repository",
                path.display()
            );
        }

        // Verify it's a git repo
        let git_check = std::process::Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match git_check {
            Ok(s) if !s.success() => {
                bail!(
                    "not a git repository: {}\nHint: provide a path to a git repository",
                    path.display()
                );
            }
            Err(e) => {
                bail!("failed to check git repo at {}: {}", path.display(), e);
            }
            _ => {}
        }

        let name = repo_input.name.clone().unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        if name.is_empty() {
            bail!("repo has empty name (path: {})", path.display());
        }

        if !names.insert(name.clone()) {
            bail!(
                "duplicate repo name: {}\nHint: use --repo /path:custom-name to disambiguate",
                name
            );
        }

        let base_branch = repo_input
            .base_branch
            .clone()
            .unwrap_or_else(|| inputs.base_branch.clone());

        resolved_repos.push(ResolvedRepo {
            path,
            name,
            base_branch,
            remote: "origin".to_string(),
        });
    }

    debug_assert!(
        worktree_base.is_absolute() || inputs.worktree_base.starts_with("~/"),
        "worktree_base should be absolute after tilde expansion"
    );
    debug_assert!(
        resolved_repos.iter().all(|r| !r.name.is_empty()),
        "all repo names must be non-empty"
    );

    Ok(ResolvedConfig {
        general: crate::config::GeneralConfig {
            worktree_base,
            base_branch: inputs.base_branch.clone(),
            branch_template: inputs.branch_template.clone(),
            username: inputs.username.clone(),
        },
        repos: resolved_repos,
    })
}

pub fn cmd_init(inputs: InitInputs, config_path: &Path, force: bool) -> Result<InitResult> {
    let resolved = validate_init_inputs(&inputs)?;
    crate::config::write_config_atomic(config_path, &resolved, force)?;

    let repos = resolved
        .repos
        .iter()
        .map(|r| InitRepoSummary {
            name: r.name.clone(),
            path: r.path.clone(),
            base_branch: r.base_branch.clone(),
        })
        .collect();

    Ok(InitResult {
        config_path: config_path.to_path_buf(),
        worktree_base: resolved.general.worktree_base,
        repos,
    })
}

pub fn format_init_human(result: &InitResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Config written to {}",
        result.config_path.display()
    ));
    lines.push(format!("Worktree base: {}", result.worktree_base.display()));
    lines.push(format!("Repos ({}): ", result.repos.len()));
    for repo in &result.repos {
        lines.push(format!(
            "  {} ({}, base: {})",
            repo.name,
            repo.path.display(),
            repo.base_branch
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_git_repo(base: &Path, name: &str) -> PathBuf {
        let repo_dir = base.join(name);
        std::fs::create_dir_all(&repo_dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_dir)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap();
        };
        run(&["init", "-b", "main"]);
        run(&["commit", "--allow-empty", "-m", "initial"]);
        repo_dir
    }

    fn make_init_inputs(repos: Vec<RepoInput>) -> InitInputs {
        InitInputs {
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            branch_template: "{user}/{name}".to_string(),
            username: "testuser".to_string(),
            repos,
        }
    }

    #[test]
    fn validate_init_valid_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "my-repo");

        let inputs = make_init_inputs(vec![RepoInput {
            path: repo.display().to_string(),
            name: None,
            base_branch: None,
        }]);

        let config = validate_init_inputs(&inputs).unwrap();
        assert_eq!(config.repos.len(), 1);
        assert_eq!(config.repos[0].name, "my-repo");
        assert_eq!(config.repos[0].base_branch, "dev");
        assert_eq!(config.general.username, "testuser");
    }

    #[test]
    fn validate_init_missing_username() {
        let inputs = InitInputs {
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            branch_template: "{user}/{name}".to_string(),
            username: String::new(),
            repos: vec![RepoInput {
                path: "/tmp/foo".to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--username"));
    }

    #[test]
    fn validate_init_empty_repos() {
        let inputs = make_init_inputs(vec![]);

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--repo"));
    }

    #[test]
    fn validate_init_duplicate_repo_names() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_a = create_test_git_repo(tmp.path(), "repo");
        let sub = tmp.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let repo_b = create_test_git_repo(&sub, "repo");

        let inputs = make_init_inputs(vec![
            RepoInput {
                path: repo_a.display().to_string(),
                name: None,
                base_branch: None,
            },
            RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            },
        ]);

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("duplicate"));
    }

    #[test]
    fn validate_init_not_a_git_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let not_git = tmp.path().join("not-git");
        std::fs::create_dir_all(&not_git).unwrap();

        let inputs = make_init_inputs(vec![RepoInput {
            path: not_git.display().to_string(),
            name: None,
            base_branch: None,
        }]);

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a git"));
    }

    #[test]
    fn validate_init_branch_template_missing_name() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");

        let inputs = InitInputs {
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            branch_template: "{user}/feature".to_string(),
            username: "testuser".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("branch-template"));
    }

    #[test]
    fn validate_init_tilde_expansion() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");

        let inputs = InitInputs {
            worktree_base: "~/worktrees".to_string(),
            base_branch: "dev".to_string(),
            branch_template: "{user}/{name}".to_string(),
            username: "testuser".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let config = validate_init_inputs(&inputs).unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            config.general.worktree_base,
            PathBuf::from(&home).join("worktrees")
        );
    }

    #[test]
    fn cmd_init_creates_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "my-repo");
        let config_path = tmp.path().join("config").join("config.toml");

        let inputs = make_init_inputs(vec![RepoInput {
            path: repo.display().to_string(),
            name: None,
            base_branch: None,
        }]);

        let result = cmd_init(inputs, &config_path, false).unwrap();
        assert_eq!(result.config_path, config_path);
        assert_eq!(result.repos.len(), 1);
        assert!(config_path.exists());

        // Verify it's valid TOML that can be parsed back
        let loaded = crate::config::load_config(&config_path).unwrap();
        assert_eq!(loaded.repos[0].name, "my-repo");
    }

    #[test]
    fn cmd_init_force_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "my-repo");
        let config_path = tmp.path().join("config").join("config.toml");

        let make = || {
            make_init_inputs(vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }])
        };

        cmd_init(make(), &config_path, false).unwrap();
        // Second call with force should succeed
        cmd_init(make(), &config_path, true).unwrap();
    }

    #[test]
    fn cmd_init_without_force_errors_on_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "my-repo");
        let config_path = tmp.path().join("config").join("config.toml");

        let make = || {
            make_init_inputs(vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }])
        };

        cmd_init(make(), &config_path, false).unwrap();
        // Second call without force should fail
        let result = cmd_init(make(), &config_path, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn validate_init_repo_path_not_exists() {
        let inputs = make_init_inputs(vec![RepoInput {
            path: "/nonexistent/repo/path".to_string(),
            name: None,
            base_branch: None,
        }]);

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }
}
