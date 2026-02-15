#![cfg(test)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

use crate::config::{ResolvedConfig, ResolvedRepo, ResolvedTemplate};
use crate::meta::{ForestMeta, ForestMode, RepoMeta};
use chrono::{DateTime, Utc};

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

        run(&["init", "-b", "main"]);
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

    /// Creates a bare repo + a regular repo with the bare as `origin`.
    /// Runs `git fetch origin` after setup so remote-tracking refs exist.
    /// Returns the path to the regular (non-bare) repo.
    pub fn create_repo_with_remote(&self, name: &str) -> PathBuf {
        let bare_path = self.dir.path().join("bare").join(format!("{}.git", name));
        let repo_path = self.dir.path().join("src").join(name);

        std::fs::create_dir_all(&bare_path).unwrap();
        std::fs::create_dir_all(&repo_path).unwrap();

        let run = |dir: &PathBuf, args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .expect("failed to run git");
            assert!(
                output.status.success(),
                "git {:?} in {} failed: {}",
                args,
                dir.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        };

        // 1. Create bare repo
        run(&bare_path, &["init", "--bare", "-b", "main"]);

        // 2. Create regular repo
        run(&repo_path, &["init", "-b", "main"]);

        // 3. Add remote
        run(
            &repo_path,
            &["remote", "add", "origin", bare_path.to_str().unwrap()],
        );

        // 4. Initial commit
        run(&repo_path, &["commit", "--allow-empty", "-m", "initial"]);

        // 5. Push main to origin
        run(&repo_path, &["push", "origin", "main"]);

        // 6. Fetch to ensure refs/remotes/origin/main exists
        run(&repo_path, &["fetch", "origin"]);

        repo_path
    }

    /// Returns a ResolvedTemplate with repos at the given names, using this env's worktree_base.
    pub fn default_template(&self, repo_names: &[&str]) -> ResolvedTemplate {
        let repos = repo_names
            .iter()
            .map(|name| ResolvedRepo {
                path: self.repo_path(name),
                name: name.to_string(),
                base_branch: "main".to_string(),
                remote: "origin".to_string(),
            })
            .collect();

        ResolvedTemplate {
            worktree_base: self.worktree_base(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos,
        }
    }

    /// Returns a ResolvedConfig with a single "default" template wrapping the given repos.
    pub fn default_config(&self, repo_names: &[&str]) -> ResolvedConfig {
        let tmpl = self.default_template(repo_names);
        let mut templates = BTreeMap::new();
        templates.insert("default".to_string(), tmpl);
        ResolvedConfig {
            default_template: "default".to_string(),
            templates,
        }
    }
}

// --- Shared test helpers ---

pub fn make_meta(
    name: &str,
    created_at: DateTime<Utc>,
    mode: ForestMode,
    repos: Vec<RepoMeta>,
) -> ForestMeta {
    ForestMeta {
        name: name.to_string(),
        created_at,
        mode,
        repos,
    }
}

pub fn make_repo(name: &str, branch: &str) -> RepoMeta {
    RepoMeta {
        name: name.to_string(),
        source: PathBuf::from(format!("/tmp/src/{}", name)),
        branch: branch.to_string(),
        base_branch: "dev".to_string(),
        branch_created: true,
    }
}

pub fn setup_forest_with_git_repos(base: &Path) -> (PathBuf, ForestMeta) {
    let forest_dir = base.join("test-forest");
    std::fs::create_dir_all(&forest_dir).unwrap();

    // Create real git repos as worktrees
    for name in &["api", "web"] {
        let repo_dir = forest_dir.join(name);
        std::fs::create_dir_all(&repo_dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
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
    }

    let meta = make_meta(
        "test-forest",
        Utc::now(),
        ForestMode::Feature,
        vec![make_repo("api", "main"), make_repo("web", "main")],
    );

    (forest_dir, meta)
}
