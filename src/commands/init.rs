use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{ResolvedConfig, ResolvedRepo, ResolvedTemplate};
use crate::paths::expand_tilde;

pub struct InitInputs {
    pub template_name: String,
    pub worktree_base: String,
    pub base_branch: String,
    pub feature_branch_template: String,
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
    pub template_name: String,
    pub worktree_base: PathBuf,
    pub repos: Vec<InitRepoSummary>,
}

#[derive(Debug, Serialize)]
pub struct InitRepoSummary {
    pub name: String,
    pub path: PathBuf,
    pub base_branch: String,
}

pub fn validate_init_inputs(inputs: &InitInputs) -> Result<ResolvedTemplate> {
    if inputs.repos.is_empty() {
        bail!("at least one --repo is required\nHint: git forest init --feature-branch-template \"yourname/{{name}}\" --repo <path>");
    }

    if !inputs.feature_branch_template.contains("{name}") {
        bail!("--feature-branch-template must contain {{name}}");
    }

    if inputs.template_name.trim().is_empty() {
        bail!("template name must not be empty");
    }
    if inputs.template_name != inputs.template_name.trim() {
        bail!("template name must not have leading/trailing whitespace");
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

    Ok(ResolvedTemplate {
        worktree_base,
        base_branch: inputs.base_branch.clone(),
        feature_branch_template: inputs.feature_branch_template.clone(),
        repos: resolved_repos,
    })
}

pub fn cmd_init(inputs: InitInputs, config_path: &Path, force: bool) -> Result<InitResult> {
    let template = validate_init_inputs(&inputs)?;
    let template_name = inputs.template_name.clone();

    let mut config = if config_path.exists() {
        crate::config::load_config(config_path)?
    } else {
        ResolvedConfig {
            default_template: template_name.clone(),
            templates: BTreeMap::new(),
        }
    };

    // Only require --force when overwriting an existing template
    if config.templates.contains_key(&template_name) && !force {
        bail!(
            "template {:?} already exists in config\n  hint: use --force to overwrite, or choose a different name",
            template_name
        );
    }

    let worktree_base = template.worktree_base.clone();
    let repos: Vec<InitRepoSummary> = template
        .repos
        .iter()
        .map(|r| InitRepoSummary {
            name: r.name.clone(),
            path: r.path.clone(),
            base_branch: r.base_branch.clone(),
        })
        .collect();

    config.templates.insert(template_name.clone(), template);
    crate::config::write_config_atomic(config_path, &config)?;

    Ok(InitResult {
        config_path: config_path.to_path_buf(),
        template_name,
        worktree_base,
        repos,
    })
}

pub fn format_init_human(result: &InitResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Config written to {}",
        result.config_path.display()
    ));
    lines.push(format!("Template: {}", result.template_name));
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
            template_name: "default".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
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

        let template = validate_init_inputs(&inputs).unwrap();
        assert_eq!(template.repos.len(), 1);
        assert_eq!(template.repos[0].name, "my-repo");
        assert_eq!(template.repos[0].base_branch, "dev");
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
            template_name: "default".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "dliv/feature".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("feature-branch-template"));
    }

    #[test]
    fn validate_init_tilde_expansion() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");

        let inputs = InitInputs {
            template_name: "default".to_string(),
            worktree_base: "~/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let template = validate_init_inputs(&inputs).unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            template.worktree_base,
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
        assert_eq!(result.template_name, "default");
        assert_eq!(result.repos.len(), 1);
        assert!(config_path.exists());

        // Verify it's valid TOML that can be parsed back
        let loaded = crate::config::load_config(&config_path).unwrap();
        let tmpl = loaded.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].name, "my-repo");
    }

    #[test]
    fn cmd_init_force_overwrites_template() {
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
    fn cmd_init_without_force_errors_on_existing_template() {
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

    // --- New template-specific tests ---

    #[test]
    fn init_adds_second_template_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_a = create_test_git_repo(tmp.path(), "repo-a");
        let repo_b = create_test_git_repo(tmp.path(), "repo-b");
        let config_path = tmp.path().join("config").join("config.toml");

        // First template
        let inputs_a = InitInputs {
            template_name: "alpha".to_string(),
            worktree_base: "/tmp/worktrees/alpha".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo_a.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_a, &config_path, false).unwrap();

        // Second template — should work without --force
        let inputs_b = InitInputs {
            template_name: "beta".to_string(),
            worktree_base: "/tmp/worktrees/beta".to_string(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_b, &config_path, false).unwrap();

        // Verify both templates exist
        let loaded = crate::config::load_config(&config_path).unwrap();
        assert_eq!(loaded.templates.len(), 2);
        assert!(loaded.templates.contains_key("alpha"));
        assert!(loaded.templates.contains_key("beta"));
    }

    #[test]
    fn init_first_template_becomes_default() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");
        let config_path = tmp.path().join("config").join("config.toml");

        let inputs = InitInputs {
            template_name: "my-project".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs, &config_path, false).unwrap();

        let loaded = crate::config::load_config(&config_path).unwrap();
        assert_eq!(loaded.default_template, "my-project");
    }

    #[test]
    fn init_second_template_does_not_change_default() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_a = create_test_git_repo(tmp.path(), "repo-a");
        let repo_b = create_test_git_repo(tmp.path(), "repo-b");
        let config_path = tmp.path().join("config").join("config.toml");

        let inputs_a = InitInputs {
            template_name: "first".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo_a.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_a, &config_path, false).unwrap();

        let inputs_b = InitInputs {
            template_name: "second".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_b, &config_path, false).unwrap();

        let loaded = crate::config::load_config(&config_path).unwrap();
        assert_eq!(loaded.default_template, "first");
    }

    #[test]
    fn init_replaces_existing_template_with_force() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_a = create_test_git_repo(tmp.path(), "repo-a");
        let repo_b = create_test_git_repo(tmp.path(), "repo-b");
        let config_path = tmp.path().join("config").join("config.toml");

        // Create template "alpha" with repo-a
        let inputs_a = InitInputs {
            template_name: "alpha".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo_a.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_a, &config_path, false).unwrap();

        // Add template "beta"
        let inputs_b = InitInputs {
            template_name: "beta".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_b, &config_path, false).unwrap();

        // Overwrite "alpha" with repo-b using --force
        let inputs_replace = InitInputs {
            template_name: "alpha".to_string(),
            worktree_base: "/tmp/worktrees/new".to_string(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_replace, &config_path, true).unwrap();

        let loaded = crate::config::load_config(&config_path).unwrap();
        assert_eq!(loaded.templates.len(), 2);
        let alpha = loaded.resolve_template(Some("alpha")).unwrap();
        assert_eq!(alpha.repos[0].name, "repo-b");
        assert_eq!(alpha.worktree_base, PathBuf::from("/tmp/worktrees/new"));
        // beta should be preserved
        assert!(loaded.templates.contains_key("beta"));
    }

    #[test]
    fn init_result_includes_template_name() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");
        let config_path = tmp.path().join("config").join("config.toml");

        let inputs = InitInputs {
            template_name: "my-project".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = cmd_init(inputs, &config_path, false).unwrap();
        assert_eq!(result.template_name, "my-project");
    }

    #[test]
    fn validate_init_template_name_empty_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");

        let inputs = InitInputs {
            template_name: "".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn validate_init_template_name_whitespace_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");

        let inputs = InitInputs {
            template_name: " spaces ".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("whitespace"));
    }
}
