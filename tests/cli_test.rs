use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;

fn bin_name() -> &'static str {
    #[cfg(feature = "beta")]
    {
        "git-forest-beta"
    }
    #[cfg(not(feature = "beta"))]
    {
        "git-forest"
    }
}

fn bin_cmd() -> assert_cmd::Command {
    #[cfg(feature = "beta")]
    {
        cargo_bin_cmd!("git-forest-beta")
    }
    #[cfg(not(feature = "beta"))]
    {
        cargo_bin_cmd!("git-forest")
    }
}

#[test]
fn help_exits_zero() {
    bin_cmd().arg("--help").assert().success();
}

#[test]
fn init_without_feature_branch_template_shows_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("my-repo");
    create_test_git_repo(&repo_dir);

    bin_cmd()
        .args(["init", "--repo", repo_dir.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--feature-branch-template"));
}

#[test]
fn init_show_path() {
    bin_cmd()
        .args(["init", "--show-path"])
        .assert()
        .success()
        .stdout(predicates::str::contains("config.toml"));
}

/// Returns the env vars needed to isolate config to a fake home directory.
/// XDG paths resolve via $HOME (~/.config/git-forest/) on Unix/macOS.
/// We clear XDG_CONFIG_HOME/XDG_STATE_HOME so the $HOME default is used.
fn config_env(fake_home: &std::path::Path) -> Vec<(&'static str, std::path::PathBuf)> {
    vec![("HOME", fake_home.to_path_buf())]
}

const CLEARED_XDG_VARS: [&str; 2] = ["XDG_CONFIG_HOME", "XDG_STATE_HOME"];

/// Returns the expected config path under a fake home directory.
fn expected_config_path(fake_home: &std::path::Path) -> std::path::PathBuf {
    fake_home
        .join(".config")
        .join(bin_name())
        .join("config.toml")
}

fn create_test_git_repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
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

#[test]
fn init_creates_config() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("my-repo");
    create_test_git_repo(&repo_dir);

    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    let expected_config = expected_config_path(&fake_home);

    let mut cmd = bin_cmd();
    cmd.args([
        "init",
        "--feature-branch-template",
        "testuser/{name}",
        "--repo",
        repo_dir.to_str().unwrap(),
        "--force",
    ]);
    for (k, v) in config_env(&fake_home) {
        cmd.env(k, v);
    }
    for k in CLEARED_XDG_VARS {
        cmd.env_remove(k);
    }
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("Config written to"));

    assert!(expected_config.exists());
}

#[test]
fn init_force_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("my-repo");
    create_test_git_repo(&repo_dir);

    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    let run_init = |force: bool| {
        let mut cmd = bin_cmd();
        cmd.args([
            "init",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_dir.to_str().unwrap(),
        ]);
        if force {
            cmd.arg("--force");
        }
        for (k, v) in config_env(&fake_home) {
            cmd.env(k, v);
        }
        for k in CLEARED_XDG_VARS {
            cmd.env_remove(k);
        }
        cmd
    };

    // First run succeeds
    run_init(false).assert().success();
    // Second run without force fails
    run_init(false)
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"));
    // Third run with force succeeds
    run_init(true).assert().success();
}

#[test]
fn init_json_output() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("my-repo");
    create_test_git_repo(&repo_dir);

    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    bin_cmd()
        .args([
            "--json",
            "init",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_dir.to_str().unwrap(),
            "--force",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("\"config_path\""))
        .stdout(predicates::str::contains("\"worktree_base\""));
}

#[test]
fn subcommand_new_requires_mode() {
    bin_cmd()
        .args(["new", "test-feature"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--mode"));
}

#[test]
fn new_without_config_shows_init_hint() {
    with_no_config()
        .args(["new", "test-feature", "--mode", "feature"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("git forest init"));
}

#[test]
fn subcommand_rm_recognized() {
    // rm without config should fail with init hint (same as other commands)
    with_no_config()
        .args(["rm", "test-feature"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("git forest init"));
}

fn with_no_config() -> assert_cmd::Command {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("empty-home");
    std::fs::create_dir_all(&fake_home).unwrap();
    let mut cmd = bin_cmd();
    for (k, v) in config_env(&fake_home) {
        cmd.env(k, v);
    }
    for k in CLEARED_XDG_VARS {
        cmd.env_remove(k);
    }
    // Keep tmpdir alive by leaking it (tests are short-lived)
    std::mem::forget(tmp);
    cmd
}

#[test]
fn ls_without_config_shows_init_hint() {
    with_no_config()
        .arg("ls")
        .assert()
        .failure()
        .stderr(predicates::str::contains("git forest init"));
}

#[test]
fn status_without_config_shows_init_hint() {
    with_no_config()
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("git forest init"));
}

#[test]
fn exec_without_config_shows_init_hint() {
    with_no_config()
        .args(["exec", "test-forest", "--", "echo", "hello"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("git forest init"));
}

#[test]
fn no_args_shows_help() {
    bin_cmd().assert().failure();
}

// --- new command integration tests ---

/// Creates a bare repo + regular repo with the bare as origin, returns the regular repo path.
fn create_repo_with_remote(base: &std::path::Path, name: &str) -> std::path::PathBuf {
    let bare_path = base.join("bare").join(format!("{}.git", name));
    let repo_path = base.join("src").join(name);

    std::fs::create_dir_all(&bare_path).unwrap();
    std::fs::create_dir_all(&repo_path).unwrap();

    let run = |dir: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} in {} failed: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&bare_path, &["init", "--bare", "-b", "main"]);
    run(&repo_path, &["init", "-b", "main"]);
    run(
        &repo_path,
        &["remote", "add", "origin", bare_path.to_str().unwrap()],
    );
    run(&repo_path, &["commit", "--allow-empty", "-m", "initial"]);
    run(&repo_path, &["push", "origin", "main"]);
    run(&repo_path, &["fetch", "origin"]);

    repo_path
}

/// Sets up a complete test environment: fake HOME, two repos with remotes, and a config.
/// Returns (tmpdir, fake_home, worktree_base).
fn setup_new_env() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    let repo_a = create_repo_with_remote(tmp.path(), "foo-api");
    let repo_b = create_repo_with_remote(tmp.path(), "foo-web");
    let worktree_base = tmp.path().join("worktrees");

    // Init config
    bin_cmd()
        .args([
            "init",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_a.to_str().unwrap(),
            "--repo",
            repo_b.to_str().unwrap(),
            "--base-branch",
            "main",
            "--worktree-base",
            worktree_base.to_str().unwrap(),
            "--force",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    (tmp, fake_home, worktree_base)
}

#[test]
fn new_feature_mode_creates_forest() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    bin_cmd()
        .args(["new", "my-feature", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("my-feature"))
        .stdout(predicates::str::contains("feature"));

    // Verify forest directory and worktrees exist
    let forest_dir = worktree_base.join("my-feature");
    assert!(forest_dir.exists());
    assert!(forest_dir.join("foo-api").exists());
    assert!(forest_dir.join("foo-web").exists());
    assert!(forest_dir.join(".forest-meta.toml").exists());

    drop(tmp);
}

#[test]
fn new_review_mode_with_repo_branch() {
    let (tmp, fake_home, _worktree_base) = setup_new_env();

    bin_cmd()
        .args([
            "new",
            "review-pr",
            "--mode",
            "review",
            "--repo-branch",
            "foo-web=custom/branch",
            "--no-fetch",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("forest/review-pr"))
        .stdout(predicates::str::contains("custom/branch"));

    drop(tmp);
}

#[test]
fn new_dry_run_does_not_create() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    bin_cmd()
        .args([
            "new",
            "dry-test",
            "--mode",
            "feature",
            "--no-fetch",
            "--dry-run",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("Dry run"))
        .stdout(predicates::str::contains("dry-test"));

    // Forest directory should NOT exist
    let forest_dir = worktree_base.join("dry-test");
    assert!(!forest_dir.exists());

    drop(tmp);
}

#[test]
fn new_json_output() {
    let (tmp, fake_home, _) = setup_new_env();

    let output = bin_cmd()
        .args([
            "--json",
            "new",
            "json-test",
            "--mode",
            "feature",
            "--no-fetch",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["forest_name"], "json-test");
    assert_eq!(json["mode"], "feature");
    assert_eq!(json["dry_run"], false);
    assert!(json["repos"].is_array());
    assert_eq!(json["repos"].as_array().unwrap().len(), 2);
    assert!(json["repos"][0]["checkout_kind"].is_string());

    drop(tmp);
}

#[test]
fn new_duplicate_forest_name_errors() {
    let (tmp, fake_home, _) = setup_new_env();

    // First create succeeds
    bin_cmd()
        .args(["new", "dup-forest", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // Second create fails
    bin_cmd()
        .args(["new", "dup-forest", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"));

    drop(tmp);
}

#[test]
fn new_no_fetch_skips_fetch() {
    let (tmp, fake_home, _) = setup_new_env();

    // --no-fetch should work even if we can't reach the remote
    bin_cmd()
        .args(["new", "no-fetch-test", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    drop(tmp);
}

#[test]
fn ls_shows_new_forest() {
    let (tmp, fake_home, _) = setup_new_env();

    bin_cmd()
        .args(["new", "visible-forest", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    bin_cmd()
        .arg("ls")
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("visible-forest"));

    drop(tmp);
}

// --- rm command integration tests ---

#[test]
fn rm_removes_forest() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    // Create forest
    bin_cmd()
        .args(["new", "rm-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    let forest_dir = worktree_base.join("rm-e2e");
    assert!(forest_dir.exists());
    assert!(forest_dir.join("foo-api").exists());
    assert!(forest_dir.join("foo-web").exists());

    // Remove forest
    bin_cmd()
        .args(["rm", "rm-e2e"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("Removing forest"));

    // Verify everything is gone
    assert!(!forest_dir.exists());
    assert!(!forest_dir.join("foo-api").exists());
    assert!(!forest_dir.join("foo-web").exists());

    drop(tmp);
}

#[test]
fn rm_dry_run_preserves_forest() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    bin_cmd()
        .args(["new", "rm-dry-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    let forest_dir = worktree_base.join("rm-dry-e2e");
    assert!(forest_dir.exists());

    // Dry run
    bin_cmd()
        .args(["rm", "rm-dry-e2e", "--dry-run"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("Dry run"))
        .stdout(predicates::str::contains("Would remove forest"));

    // Everything should still exist
    assert!(forest_dir.exists());
    assert!(forest_dir.join("foo-api").exists());
    assert!(forest_dir.join(".forest-meta.toml").exists());

    drop(tmp);
}

#[test]
fn rm_json_output() {
    let (tmp, fake_home, _) = setup_new_env();

    bin_cmd()
        .args(["new", "rm-json-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    let output = bin_cmd()
        .args(["--json", "rm", "rm-json-e2e"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["forest_name"], "rm-json-e2e");
    assert_eq!(json["dry_run"], false);
    assert!(json["repos"].is_array());
    assert_eq!(json["repos"].as_array().unwrap().len(), 2);
    assert!(json["forest_dir_removed"].as_bool().unwrap());
    assert!(json["errors"].as_array().unwrap().is_empty());

    drop(tmp);
}

#[test]
fn rm_nonexistent_forest_errors() {
    let (_tmp, fake_home, _) = setup_new_env();

    bin_cmd()
        .args(["rm", "does-not-exist"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .failure()
        .stderr(predicates::str::contains("not found"));
}

#[test]
fn rm_force_flag() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    bin_cmd()
        .args(["new", "rm-force-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    let forest_dir = worktree_base.join("rm-force-e2e");

    // Make foo-api dirty
    let dirty_file = forest_dir.join("foo-api").join("dirty.txt");
    std::fs::write(&dirty_file, "dirty content").unwrap();
    let run = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
    };
    run(&forest_dir.join("foo-api"), &["add", "dirty.txt"]);

    // Without --force: should exit 1 (partial failure)
    bin_cmd()
        .args(["rm", "rm-force-e2e"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .failure();

    // Re-create (the forest dir still exists with leftover worktree)
    // Need to clean up and recreate
    std::fs::remove_dir_all(&forest_dir).unwrap();
    // Also prune worktrees in source repos
    run(
        &tmp.path().join("src").join("foo-api"),
        &["worktree", "prune"],
    );
    run(
        &tmp.path().join("src").join("foo-web"),
        &["worktree", "prune"],
    );

    bin_cmd()
        .args(["new", "rm-force-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // Make dirty again
    let dirty_file = forest_dir.join("foo-api").join("dirty2.txt");
    std::fs::write(&dirty_file, "dirty content 2").unwrap();
    run(&forest_dir.join("foo-api"), &["add", "dirty2.txt"]);

    // With --force: should succeed
    bin_cmd()
        .args(["rm", "rm-force-e2e", "--force"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("Removing forest"));

    assert!(!forest_dir.exists());

    drop(tmp);
}

// --- multi-template integration tests ---

#[test]
fn init_with_template_name() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("my-repo");
    create_test_git_repo(&repo_dir);

    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    bin_cmd()
        .args([
            "init",
            "--template",
            "my-project",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_dir.to_str().unwrap(),
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("Template: my-project"));

    drop(tmp);
}

#[test]
fn new_with_template_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    let repo_a = create_repo_with_remote(tmp.path(), "alpha-api");
    let repo_b = create_repo_with_remote(tmp.path(), "beta-api");
    let wt_base = tmp.path().join("worktrees");

    // Create template "alpha" with alpha-api
    bin_cmd()
        .args([
            "init",
            "--template",
            "alpha",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_a.to_str().unwrap(),
            "--base-branch",
            "main",
            "--worktree-base",
            wt_base.to_str().unwrap(),
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // Add template "beta" with beta-api
    bin_cmd()
        .args([
            "init",
            "--template",
            "beta",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_b.to_str().unwrap(),
            "--base-branch",
            "main",
            "--worktree-base",
            wt_base.to_str().unwrap(),
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // Create forest using --template beta → should only have beta-api
    bin_cmd()
        .args([
            "new",
            "beta-feature",
            "--mode",
            "feature",
            "--template",
            "beta",
            "--no-fetch",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("beta-api"));

    // Forest should exist with only beta-api worktree
    let forest_dir = wt_base.join("beta-feature");
    assert!(forest_dir.join("beta-api").exists());
    assert!(!forest_dir.join("alpha-api").exists());

    drop(tmp);
}

#[test]
fn multi_template_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    let repo_alpha = create_repo_with_remote(tmp.path(), "foo-api");
    let repo_beta = create_repo_with_remote(tmp.path(), "foo-web");

    let wt_alpha = tmp.path().join("worktrees").join("alpha");
    let wt_beta = tmp.path().join("worktrees").join("beta");

    // init --template alpha with repo foo-api
    bin_cmd()
        .args([
            "init",
            "--template",
            "alpha",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_alpha.to_str().unwrap(),
            "--base-branch",
            "main",
            "--worktree-base",
            wt_alpha.to_str().unwrap(),
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // init --template beta with repo foo-web
    bin_cmd()
        .args([
            "init",
            "--template",
            "beta",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_beta.to_str().unwrap(),
            "--base-branch",
            "main",
            "--worktree-base",
            wt_beta.to_str().unwrap(),
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // new alpha-feature --mode feature --template alpha --no-fetch
    bin_cmd()
        .args([
            "new",
            "alpha-feature",
            "--mode",
            "feature",
            "--template",
            "alpha",
            "--no-fetch",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // new beta-feature --mode feature --template beta --no-fetch
    bin_cmd()
        .args([
            "new",
            "beta-feature",
            "--mode",
            "feature",
            "--template",
            "beta",
            "--no-fetch",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // ls → both forests visible
    bin_cmd()
        .arg("ls")
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("alpha-feature"))
        .stdout(predicates::str::contains("beta-feature"));

    // rm alpha-feature
    bin_cmd()
        .args(["rm", "alpha-feature"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // ls → only beta-feature remains
    bin_cmd()
        .arg("ls")
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("beta-feature"))
        .stdout(predicates::str::contains("alpha-feature").not());

    // rm beta-feature
    bin_cmd()
        .args(["rm", "beta-feature"])
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success();

    // ls → empty
    bin_cmd()
        .arg("ls")
        .env("HOME", fake_home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .assert()
        .success()
        .stdout(predicates::str::contains("No forests found"));

    drop(tmp);
}

// --- non-blocking version check integration tests ---

fn write_version_state(
    fake_home: &std::path::Path,
    last_checked: &str,
    latest_version: Option<&str>,
) {
    let state_dir = fake_home.join(".local").join("state").join(bin_name());
    std::fs::create_dir_all(&state_dir).unwrap();
    let mut content = format!("[version_check]\nlast_checked = \"{}\"\n", last_checked);
    if let Some(v) = latest_version {
        content.push_str(&format!("latest_version = \"{}\"\n", v));
    }
    std::fs::write(state_dir.join("state.toml"), content).unwrap();
}

fn read_version_state(fake_home: &std::path::Path) -> Option<String> {
    let path = fake_home
        .join(".local")
        .join("state")
        .join(bin_name())
        .join("state.toml");
    std::fs::read_to_string(path).ok()
}

#[test]
fn version_check_fresh_cache_no_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // Cache is fresh with same version as current — no notice expected
    write_version_state(
        &fake_home,
        "2099-01-01T00:00:00Z",
        Some(env!("CARGO_PKG_VERSION")),
    );

    let output = bin_cmd()
        .args(["init", "--show-path"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("Update available"),
        "unexpected update notice: {}",
        stderr
    );
    assert!(
        !stderr.contains("checks for updates daily"),
        "unexpected first-run notice: {}",
        stderr
    );
}

#[test]
fn version_check_cached_newer_version_shows_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // Cache has a newer version — should print notice from cache (no network)
    write_version_state(&fake_home, "2099-01-01T00:00:00Z", Some("99.99.99"));

    let output = bin_cmd()
        .args(["init", "--show-path"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("Update available"),
        "expected update notice: {}",
        stderr
    );
    assert!(
        stderr.contains("99.99.99"),
        "expected version in notice: {}",
        stderr
    );
}

#[test]
fn version_check_stale_cache_updates_timestamp() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // Cache is stale — should update last_checked and spawn background check
    write_version_state(
        &fake_home,
        "2020-01-01T00:00:00Z",
        Some(env!("CARGO_PKG_VERSION")),
    );

    let output = bin_cmd()
        .args(["init", "--show-path"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();

    assert!(output.status.success());

    // last_checked should have been updated (no longer 2020)
    let state = read_version_state(&fake_home).unwrap();
    assert!(
        !state.contains("2020-01-01"),
        "last_checked should have been updated: {}",
        state
    );

    // No update notice (same version cached)
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("Update available"),
        "no update notice expected: {}",
        stderr
    );
}

#[test]
fn version_check_stale_cache_with_newer_version_shows_notice_and_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // Cache is stale AND has newer version — should show notice AND update timestamp
    write_version_state(&fake_home, "2020-01-01T00:00:00Z", Some("99.99.99"));

    let output = bin_cmd()
        .args(["init", "--show-path"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("Update available"),
        "expected update notice: {}",
        stderr
    );
    assert!(
        stderr.contains("99.99.99"),
        "expected version in notice: {}",
        stderr
    );

    // Timestamp should have been updated
    let state = read_version_state(&fake_home).unwrap();
    assert!(
        !state.contains("2020-01-01"),
        "last_checked should have been updated: {}",
        state
    );
}

#[test]
fn version_check_first_run_shows_privacy_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // No state file — should show privacy notice and create state
    let output = bin_cmd()
        .args(["init", "--show-path"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("checks for updates daily"),
        "expected privacy notice: {}",
        stderr
    );

    // State file should have been created (even if network failed)
    let state = read_version_state(&fake_home);
    assert!(state.is_some(), "state file should have been created");
}

#[test]
fn version_check_missing_latest_version_does_sync_check() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // State exists but latest_version is missing — triggers background check
    write_version_state(&fake_home, "2099-01-01T00:00:00Z", None);

    let output = bin_cmd()
        .args(["init", "--show-path"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();

    assert!(output.status.success());

    // Should NOT show privacy notice (state file existed)
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("checks for updates daily"),
        "should not show privacy notice when state file exists: {}",
        stderr
    );

    // State file should still exist with last_checked updated
    let state = read_version_state(&fake_home);
    assert!(state.is_some(), "state file should still exist");

    // last_checked should have been updated (no longer far-future)
    let state_content = state.unwrap();
    assert!(
        !state_content.contains("2099-01-01"),
        "last_checked should have been updated: {}",
        state_content
    );
}

#[test]
fn internal_version_check_flag_exits_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    write_version_state(&fake_home, "2020-01-01T00:00:00Z", Some("0.0.1"));

    let output = bin_cmd()
        .arg("--internal-version-check")
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "subprocess should produce no stdout"
    );
    assert!(
        output.stderr.is_empty(),
        "subprocess should produce no stderr"
    );
}

#[test]
fn version_check_no_ambiguous_message() {
    // Ensure the old confusing "or the update server is unreachable" message is gone
    let output = bin_cmd().args(["version", "--check"]).output().unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("or the update server is unreachable"),
        "old ambiguous message should be gone: {}",
        stderr
    );
}

#[test]
fn reset_confirm_does_not_trigger_version_check() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // Write a config so reset has something to delete
    let config_dir = fake_home.join(".config").join(bin_name());
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
default_template = "default"
[template.default]
worktree_base = "/tmp/nonexistent"
base_branch = "main"
feature_branch_template = "test/{name}"
[[template.default.repos]]
path = "/tmp/nonexistent-repo"
"#,
    )
    .unwrap();

    // Write a version state file
    write_version_state(&fake_home, "2020-01-01T00:00:00Z", Some("0.0.1"));

    bin_cmd()
        .args(["reset", "--confirm"])
        .env("HOME", &fake_home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .assert()
        .success();

    // State file should be gone (deleted by reset) and NOT recreated by version check
    assert!(
        read_version_state(&fake_home).is_none(),
        "version check should not run after reset"
    );
}

// --- version / update command integration tests ---

#[test]
fn version_flag_outputs_version() {
    bin_cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains(format!("{} 0.", bin_name())));
}

#[test]
fn version_subcommand_outputs_version() {
    bin_cmd()
        .arg("version")
        .assert()
        .success()
        .stdout(predicates::str::contains(format!("{} 0.", bin_name())));
}

#[test]
fn version_check_graceful_failure() {
    // --check should succeed even when the endpoint is unreachable
    bin_cmd()
        .args(["version", "--check"])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!("{} 0.", bin_name())));
}

#[test]
fn debug_version_check_shows_debug_output() {
    let output = bin_cmd()
        .args(["--debug", "version", "--check"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[debug]"),
        "expected debug output on stderr, got: {}",
        stderr
    );
}

#[test]
fn update_command_does_not_crash() {
    // Should print either brew output or download link
    bin_cmd().arg("update").assert().success();
}
