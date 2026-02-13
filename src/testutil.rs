#![cfg(test)]

use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

use crate::config::{GeneralConfig, RepoConfig, ResolvedConfig, ResolvedRepo};

pub struct TestEnv {
    dir: TempDir,
}

impl TestEnv {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp dir");
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("worktrees")).unwrap();
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        Self { dir }
    }

    pub fn create_repo(&self, name: &str) -> PathBuf {
        let repo_path = self.dir.path().join("src").join(name);
        std::fs::create_dir_all(&repo_path).unwrap();

        let run = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo_path)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .expect("failed to run git");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };

        run(&["init"]);
        run(&["commit", "--allow-empty", "-m", "initial"]);

        repo_path
    }

    pub fn create_repo_with_branch(&self, name: &str, branch: &str) -> PathBuf {
        let repo_path = self.create_repo(name);

        let output = Command::new("git")
            .args(["checkout", "-b", branch])
            .current_dir(&repo_path)
            .output()
            .expect("failed to run git checkout");
        assert!(
            output.status.success(),
            "git checkout -b {} failed: {}",
            branch,
            String::from_utf8_lossy(&output.stderr)
        );

        repo_path
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.path().join("config").join("config.toml")
    }

    pub fn src_dir(&self) -> PathBuf {
        self.dir.path().join("src")
    }

    pub fn worktree_base(&self) -> PathBuf {
        self.dir.path().join("worktrees")
    }

    pub fn repo_path(&self, name: &str) -> PathBuf {
        self.dir.path().join("src").join(name)
    }

    pub fn write_config(&self, config: &ResolvedConfig) {
        let raw = crate::config::Config {
            general: config.general.clone(),
            repos: config
                .repos
                .iter()
                .map(|r| RepoConfig {
                    path: r.path.clone(),
                    name: Some(r.name.clone()),
                    base_branch: Some(r.base_branch.clone()),
                    remote: Some(r.remote.clone()),
                })
                .collect(),
        };
        let content = toml::to_string_pretty(&raw).expect("failed to serialize config");
        std::fs::write(self.config_path(), content).expect("failed to write config");
    }

    pub fn default_config(&self, repo_names: &[&str]) -> ResolvedConfig {
        let repos = repo_names
            .iter()
            .map(|name| ResolvedRepo {
                path: self.repo_path(name),
                name: name.to_string(),
                base_branch: "main".to_string(),
                remote: "origin".to_string(),
            })
            .collect();

        ResolvedConfig {
            general: GeneralConfig {
                worktree_base: self.worktree_base(),
                base_branch: "main".to_string(),
                branch_template: "{user}/{name}".to_string(),
                username: "testuser".to_string(),
            },
            repos,
        }
    }
}
