use anyhow::Result;
use std::path::Path;

use crate::config::ResolvedTemplate;
use crate::git::is_git_repository;
use git_forest_core::config::expand_tilde;
use git_forest_core::plan::init::{
    decide_init, merge_init, validate_init_inputs, InitObservation, InitRepoSummary, InitResult,
    RepoPathObservation,
};
pub use git_forest_core::plan::init::{format_init_human, InitInputs, RepoInput};

/// Reads what `init` needs once the inputs are valid: the worktree base, expanded with `home`,
/// and for each `--repo` path in input order whether it expands, whether something is there and
/// whether git finds a repository there. Every path is observed; every error is kept for
/// `decide_init`.
fn observe_init(inputs: &InitInputs, home: Option<&str>) -> InitObservation {
    InitObservation {
        worktree_base: expand_tilde(&inputs.worktree_base, home),
        repos: inputs
            .repos
            .iter()
            .map(|repo_input| observe_repo_path(&repo_input.path, home))
            .collect(),
    }
}

fn observe_repo_path(path: &str, home: Option<&str>) -> RepoPathObservation {
    let path = match expand_tilde(path, home) {
        Ok(path) => path,
        Err(error) => return RepoPathObservation::Unexpanded(error),
    };
    if !path.exists() {
        return RepoPathObservation::Missing(path);
    }
    match is_git_repository(&path) {
        Ok(true) => RepoPathObservation::Repository(path),
        Ok(false) => RepoPathObservation::NotRepository(path),
        Err(error) => RepoPathObservation::CheckFailed {
            path,
            error: error.to_string(),
        },
    }
}

/// Resolves the template `init` adds: the input checks, then the observation, then the
/// decision over it. `~` paths expand with `home`.
fn plan_init(inputs: &InitInputs, home: Option<&str>) -> Result<ResolvedTemplate> {
    validate_init_inputs(inputs)?;
    decide_init(inputs, observe_init(inputs, home))
}

/// Adds the template the inputs describe to the config file at `config_path`, which the
/// composition root resolved, creating the file when it does not exist; `~` paths, in the
/// inputs and in the existing config, expand with `home`.
pub fn cmd_init(
    inputs: InitInputs,
    config_path: &Path,
    force: bool,
    home: Option<&str>,
) -> Result<InitResult> {
    let template = plan_init(&inputs, home)?;
    let template_name = inputs.template_name.clone();

    let existing = if config_path.exists() {
        Some(crate::config::load_config(config_path, home)?)
    } else {
        None
    };

    let worktree_base = template.worktree_base.clone();
    let disposable_root_entries = template.disposable_root_entries.clone();
    let repos: Vec<InitRepoSummary> = template
        .repos
        .iter()
        .map(|r| InitRepoSummary {
            name: r.name.to_string(),
            path: r.path.clone(),
            base_branch: r.base_branch.clone(),
        })
        .collect();

    let config = merge_init(existing, &template_name, template, force)?;
    crate::config::write_config_atomic(config_path, &config)?;

    Ok(InitResult {
        config_path: config_path.to_path_buf(),
        template_name,
        worktree_base,
        disposable_root_entries,
        repos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_forest_core::paths::DisposableRootEntry;
    use std::path::PathBuf;

    /// A fixed fake home directory, so that an expanded `~` proves the passed value is used.
    const TEST_HOME: &str = "/home/tester";

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
            disposable_root_entries: vec![],
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

        let template = plan_init(&inputs, Some(TEST_HOME)).unwrap();
        assert_eq!(template.repos.len(), 1);
        assert_eq!(template.repos[0].name.as_str(), "my-repo");
        assert_eq!(template.repos[0].base_branch, "dev");
    }

    #[test]
    fn validate_init_accepts_disposable_root_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo-a");
        let mut inputs = make_init_inputs(vec![RepoInput {
            path: repo.display().to_string(),
            name: None,
            base_branch: None,
        }]);
        inputs.disposable_root_entries = vec![
            DisposableRootEntry::new(".idea".to_string()).unwrap(),
            DisposableRootEntry::new(".claude".to_string()).unwrap(),
        ];

        let template = plan_init(&inputs, Some(TEST_HOME)).unwrap();

        assert_eq!(
            template.disposable_root_entries,
            inputs.disposable_root_entries
        );
    }

    #[test]
    fn validate_init_rejects_disposable_managed_repo_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo-a");
        let mut inputs = make_init_inputs(vec![RepoInput {
            path: repo.display().to_string(),
            name: None,
            base_branch: None,
        }]);
        inputs.disposable_root_entries =
            vec![DisposableRootEntry::new("repo-a".to_string()).unwrap()];

        let error = plan_init(&inputs, Some(TEST_HOME)).unwrap_err().to_string();

        assert!(error.contains("reserved"), "error: {error}");
    }

    #[test]
    fn validate_init_empty_repos() {
        let inputs = make_init_inputs(vec![]);

        let result = plan_init(&inputs, Some(TEST_HOME));
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

        let result = plan_init(&inputs, Some(TEST_HOME));
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

        let result = plan_init(&inputs, Some(TEST_HOME));
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
            feature_branch_template: "user/feature".to_string(),
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = plan_init(&inputs, Some(TEST_HOME));
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
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let template = plan_init(&inputs, Some(TEST_HOME)).unwrap();
        assert_eq!(
            *template.worktree_base,
            *PathBuf::from(TEST_HOME).join("worktrees")
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

        let result = cmd_init(inputs, &config_path, false, Some(TEST_HOME)).unwrap();
        assert_eq!(result.config_path, config_path);
        assert_eq!(result.template_name, "default");
        assert_eq!(result.repos.len(), 1);
        assert!(config_path.exists());

        // Verify it's valid TOML that can be parsed back
        let loaded = crate::config::load_config(&config_path, Some(TEST_HOME)).unwrap();
        let tmpl = loaded.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].name.as_str(), "my-repo");
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

        cmd_init(make(), &config_path, false, Some(TEST_HOME)).unwrap();
        // Second call with force should succeed
        cmd_init(make(), &config_path, true, Some(TEST_HOME)).unwrap();
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

        cmd_init(make(), &config_path, false, Some(TEST_HOME)).unwrap();
        // Second call without force should fail
        let result = cmd_init(make(), &config_path, false, Some(TEST_HOME));
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

        let result = plan_init(&inputs, Some(TEST_HOME));
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
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo_a.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_a, &config_path, false, Some(TEST_HOME)).unwrap();

        // Second template — should work without --force
        let inputs_b = InitInputs {
            template_name: "beta".to_string(),
            worktree_base: "/tmp/worktrees/beta".to_string(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_b, &config_path, false, Some(TEST_HOME)).unwrap();

        // Verify both templates exist
        let loaded = crate::config::load_config(&config_path, Some(TEST_HOME)).unwrap();
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
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs, &config_path, false, Some(TEST_HOME)).unwrap();

        let loaded = crate::config::load_config(&config_path, Some(TEST_HOME)).unwrap();
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
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo_a.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_a, &config_path, false, Some(TEST_HOME)).unwrap();

        let inputs_b = InitInputs {
            template_name: "second".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_b, &config_path, false, Some(TEST_HOME)).unwrap();

        let loaded = crate::config::load_config(&config_path, Some(TEST_HOME)).unwrap();
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
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo_a.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_a, &config_path, false, Some(TEST_HOME)).unwrap();

        // Add template "beta"
        let inputs_b = InitInputs {
            template_name: "beta".to_string(),
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_b, &config_path, false, Some(TEST_HOME)).unwrap();

        // Overwrite "alpha" with repo-b using --force
        let inputs_replace = InitInputs {
            template_name: "alpha".to_string(),
            worktree_base: "/tmp/worktrees/new".to_string(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };
        cmd_init(inputs_replace, &config_path, true, Some(TEST_HOME)).unwrap();

        let loaded = crate::config::load_config(&config_path, Some(TEST_HOME)).unwrap();
        assert_eq!(loaded.templates.len(), 2);
        let alpha = loaded.resolve_template(Some("alpha")).unwrap();
        assert_eq!(alpha.repos[0].name.as_str(), "repo-b");
        assert_eq!(*alpha.worktree_base, *PathBuf::from("/tmp/worktrees/new"));
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
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = cmd_init(inputs, &config_path, false, Some(TEST_HOME)).unwrap();
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
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = plan_init(&inputs, Some(TEST_HOME));
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
            disposable_root_entries: vec![],
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = plan_init(&inputs, Some(TEST_HOME));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("whitespace"));
    }
}
