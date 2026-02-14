use assert_cmd::cargo_bin_cmd;

#[test]
fn help_exits_zero() {
    cargo_bin_cmd!("git-forest")
        .arg("--help")
        .assert()
        .success();
}

#[test]
fn init_without_username_shows_hint() {
    cargo_bin_cmd!("git-forest")
        .arg("init")
        .assert()
        .failure()
        .stderr(predicates::str::contains("--username"));
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
            "--username",
            "testuser",
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
            "--username",
            "testuser",
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
            "--username",
            "testuser",
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
    cargo_bin_cmd!("git-forest")
        .args(["rm", "test-feature"])
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
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
