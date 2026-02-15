use assert_cmd::cargo_bin_cmd;

#[test]
fn help_exits_zero() {
    cargo_bin_cmd!("git-forest")
        .arg("--help")
        .assert()
        .success();
}

#[test]
fn init_without_feature_branch_template_shows_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("my-repo");
    create_test_git_repo(&repo_dir);

    cargo_bin_cmd!("git-forest")
        .args(["init", "--repo", repo_dir.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--feature-branch-template"));
}

#[test]
fn init_show_path() {
    cargo_bin_cmd!("git-forest")
        .args(["init", "--show-path"])
        .assert()
        .success()
        .stdout(predicates::str::contains("config.toml"));
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

    // Use HOME override so directories crate writes to our temp dir
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();

    // On macOS: ~/Library/Application Support/git-forest/config.toml
    let expected_config = fake_home
        .join("Library")
        .join("Application Support")
        .join("git-forest")
        .join("config.toml");

    cargo_bin_cmd!("git-forest")
        .args([
            "init",
            "--feature-branch-template",
            "testuser/{name}",
            "--repo",
            repo_dir.to_str().unwrap(),
            "--force",
        ])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
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
        let mut cmd = cargo_bin_cmd!("git-forest");
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
        cmd.env("HOME", fake_home.to_str().unwrap());
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

    cargo_bin_cmd!("git-forest")
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
        .assert()
        .success()
        .stdout(predicates::str::contains("\"config_path\""))
        .stdout(predicates::str::contains("\"worktree_base\""));
}

#[test]
fn subcommand_new_requires_mode() {
    cargo_bin_cmd!("git-forest")
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
    let mut cmd = cargo_bin_cmd!("git-forest");
    cmd.env("HOME", fake_home.to_str().unwrap());
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
    cargo_bin_cmd!("git-forest").assert().failure();
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
    cargo_bin_cmd!("git-forest")
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
        .assert()
        .success();

    (tmp, fake_home, worktree_base)
}

#[test]
fn new_feature_mode_creates_forest() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    cargo_bin_cmd!("git-forest")
        .args(["new", "my-feature", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
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

    cargo_bin_cmd!("git-forest")
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
        .assert()
        .success()
        .stdout(predicates::str::contains("forest/review-pr"))
        .stdout(predicates::str::contains("custom/branch"));

    drop(tmp);
}

#[test]
fn new_dry_run_does_not_create() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    cargo_bin_cmd!("git-forest")
        .args([
            "new",
            "dry-test",
            "--mode",
            "feature",
            "--no-fetch",
            "--dry-run",
        ])
        .env("HOME", fake_home.to_str().unwrap())
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

    let output = cargo_bin_cmd!("git-forest")
        .args([
            "--json",
            "new",
            "json-test",
            "--mode",
            "feature",
            "--no-fetch",
        ])
        .env("HOME", fake_home.to_str().unwrap())
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
    cargo_bin_cmd!("git-forest")
        .args(["new", "dup-forest", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success();

    // Second create fails
    cargo_bin_cmd!("git-forest")
        .args(["new", "dup-forest", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"));

    drop(tmp);
}

#[test]
fn new_no_fetch_skips_fetch() {
    let (tmp, fake_home, _) = setup_new_env();

    // --no-fetch should work even if we can't reach the remote
    cargo_bin_cmd!("git-forest")
        .args(["new", "no-fetch-test", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success();

    drop(tmp);
}

#[test]
fn ls_shows_new_forest() {
    let (tmp, fake_home, _) = setup_new_env();

    cargo_bin_cmd!("git-forest")
        .args(["new", "visible-forest", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success();

    cargo_bin_cmd!("git-forest")
        .arg("ls")
        .env("HOME", fake_home.to_str().unwrap())
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
    cargo_bin_cmd!("git-forest")
        .args(["new", "rm-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success();

    let forest_dir = worktree_base.join("rm-e2e");
    assert!(forest_dir.exists());
    assert!(forest_dir.join("foo-api").exists());
    assert!(forest_dir.join("foo-web").exists());

    // Remove forest
    cargo_bin_cmd!("git-forest")
        .args(["rm", "rm-e2e"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicates::str::contains("Removed forest"));

    // Verify everything is gone
    assert!(!forest_dir.exists());
    assert!(!forest_dir.join("foo-api").exists());
    assert!(!forest_dir.join("foo-web").exists());

    drop(tmp);
}

#[test]
fn rm_dry_run_preserves_forest() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    cargo_bin_cmd!("git-forest")
        .args(["new", "rm-dry-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success();

    let forest_dir = worktree_base.join("rm-dry-e2e");
    assert!(forest_dir.exists());

    // Dry run
    cargo_bin_cmd!("git-forest")
        .args(["rm", "rm-dry-e2e", "--dry-run"])
        .env("HOME", fake_home.to_str().unwrap())
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

    cargo_bin_cmd!("git-forest")
        .args(["new", "rm-json-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success();

    let output = cargo_bin_cmd!("git-forest")
        .args(["--json", "rm", "rm-json-e2e"])
        .env("HOME", fake_home.to_str().unwrap())
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

    cargo_bin_cmd!("git-forest")
        .args(["rm", "does-not-exist"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .failure()
        .stderr(predicates::str::contains("not found"));
}

#[test]
fn rm_force_flag() {
    let (tmp, fake_home, worktree_base) = setup_new_env();

    cargo_bin_cmd!("git-forest")
        .args(["new", "rm-force-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
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
    cargo_bin_cmd!("git-forest")
        .args(["rm", "rm-force-e2e"])
        .env("HOME", fake_home.to_str().unwrap())
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

    cargo_bin_cmd!("git-forest")
        .args(["new", "rm-force-e2e", "--mode", "feature", "--no-fetch"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success();

    // Make dirty again
    let dirty_file = forest_dir.join("foo-api").join("dirty2.txt");
    std::fs::write(&dirty_file, "dirty content 2").unwrap();
    run(&forest_dir.join("foo-api"), &["add", "dirty2.txt"]);

    // With --force: should succeed
    cargo_bin_cmd!("git-forest")
        .args(["rm", "rm-force-e2e", "--force"])
        .env("HOME", fake_home.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicates::str::contains("Removed forest"));

    assert!(!forest_dir.exists());

    drop(tmp);
}
