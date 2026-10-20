//! A test-only second composition root over the forest ops crate's public API. It wires that
//! API from explicit values, as a second host would, and asserts that each value is used: the
//! file locations come from the given environment, the config's `~` paths expand with the given
//! home, and a forest is created, listed and resolved from the given working directory. It reads
//! no process environment, so it passes with the environment emptied. It does not cover the
//! update check's background handshake, which lives in the root, or `init` and `reset` beyond
//! config loading.
#![allow(
    clippy::disallowed_methods,
    reason = "a test-only composition root: it builds real git repositories to drive the forest ops crate's public API"
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, TimeZone, Utc};
use git_forest_core::meta::ForestMode;
use git_forest_ops::{commands, config, forest};

const APP_NAME: &str = "git-forest";

/// A config whose paths start at the home directory, with the update check disabled.
const CONFIG: &str = r#"default_template = "default"

[template.default]
worktree_base = "~/worktrees"
base_branch = "main"
feature_branch_template = "testuser/{name}"

[[template.default.repos]]
path = "~/src/foo-api"

[[template.default.repos]]
path = "~/src/foo-web"

[version_check]
enabled = false
"#;

/// A fixed time, so no test reads the clock.
fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

/// Runs git in `dir` with an explicit identity: under an emptied environment git has no `HOME`,
/// so nothing else would supply one.
fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A source repository at `<home>/src/<name>` whose bare remote under `<root>/remotes` has `main`
/// pushed, as `new` needs.
fn source_repository(root: &Path, home: &Path, name: &str) {
    let repo = home.join("src").join(name);
    let remote = root.join("remotes").join(format!("{name}.git"));
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["fetch", "-q", "origin"]);
}

fn some(value: &str) -> Option<String> {
    Some(value.to_string())
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn locations_come_from_the_given_environment() {
    let explicit = config::Environment {
        home: some("/fake/home"),
        xdg_config_home: some("/fake/config"),
        xdg_state_home: some("/fake/state"),
    };
    assert_eq!(
        config::default_config_path(APP_NAME, &explicit).unwrap(),
        PathBuf::from("/fake/config/git-forest/config.toml")
    );
    assert_eq!(
        config::xdg_state_dir(APP_NAME, &explicit).unwrap(),
        PathBuf::from("/fake/state/git-forest")
    );

    // Without the XDG variables, or with them empty, the locations follow the given home.
    #[cfg(unix)]
    for xdg in [None, some("")] {
        let from_home = config::Environment {
            home: some("/fake/home"),
            xdg_config_home: xdg.clone(),
            xdg_state_home: xdg,
        };
        assert_eq!(
            config::default_config_path(APP_NAME, &from_home).unwrap(),
            PathBuf::from("/fake/home/.config/git-forest/config.toml")
        );
        assert_eq!(
            config::xdg_state_dir(APP_NAME, &from_home).unwrap(),
            PathBuf::from("/fake/home/.local/state/git-forest")
        );
    }
}

#[test]
fn config_loads_with_the_given_home() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let path = root.path().join("config.toml");
    std::fs::write(&path, CONFIG).unwrap();

    let loaded = config::load_default_config(&path, home.to_str()).unwrap();
    let template = loaded.resolve_template(None).unwrap();
    assert_eq!(*template.worktree_base, *home.join("worktrees"));
    assert_eq!(*template.repos[0].path, *home.join("src").join("foo-api"));

    // Without a home, `~` does not expand, whatever the process's own environment holds.
    let error = config::load_default_config(&path, None).unwrap_err();
    assert!(
        format!("{error:#}").contains("cannot expand ~/: HOME environment variable is not set"),
        "{error:#}"
    );
}

#[test]
fn a_forest_is_created_listed_and_resolved_from_explicit_values() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let home = root.join("home");
    for name in ["foo-api", "foo-web"] {
        source_repository(root, &home, name);
    }
    let environment = config::Environment {
        home: home.to_str().map(str::to_string),
        xdg_config_home: root.join("xdg-config").to_str().map(str::to_string),
        xdg_state_home: root.join("xdg-state").to_str().map(str::to_string),
    };

    let config_path = config::default_config_path(APP_NAME, &environment).unwrap();
    assert!(config_path.starts_with(root.join("xdg-config")));
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, CONFIG).unwrap();
    let loaded = config::load_default_config(&config_path, environment.home.as_deref()).unwrap();
    let template = loaded.resolve_template(None).unwrap();

    let inputs = commands::NewInputs {
        name: "feat".to_string(),
        mode: ForestMode::Feature,
        branch_override: None,
        repo_branches: vec![],
        no_fetch: true,
        dry_run: false,
    };
    let created = commands::cmd_new_at(inputs, template, fixed_now()).unwrap();
    let forest_dir = home.join("worktrees").join("feat");
    assert_eq!(canonical(&created.forest_dir), canonical(&forest_dir));

    let bases = loaded.all_worktree_bases();
    let listed = commands::cmd_ls(&bases, fixed_now()).unwrap();
    let names: Vec<&str> = listed
        .forests
        .iter()
        .map(|forest| forest.name.as_str())
        .collect();
    assert_eq!(names, ["feat"]);

    // The given working directory decides name-less resolution: the process's own, the crate
    // directory under `cargo test`, is in no forest.
    let inside = forest::WorkingDirectory {
        current: Ok(forest_dir.join("foo-api")),
        pwd: None,
    };
    let (dir, meta) = forest::resolve_forest_multi(&bases, None, inside).unwrap();
    assert_eq!(meta.name.as_str(), "feat");
    assert_eq!(canonical(&dir), canonical(&forest_dir));

    let outside = forest::WorkingDirectory {
        current: Ok(root.to_path_buf()),
        pwd: None,
    };
    let error = forest::resolve_forest_multi(&bases, None, outside).unwrap_err();
    assert!(
        error.to_string().contains("not inside a forest directory"),
        "{error:#}"
    );
}
