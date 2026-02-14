use assert_cmd::cargo_bin_cmd;

#[test]
fn help_exits_zero() {
    cargo_bin_cmd!("git-forest")
        .arg("--help")
        .assert()
        .success();
}

#[test]
fn subcommand_init_recognized() {
    cargo_bin_cmd!("git-forest")
        .arg("init")
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_new_recognized() {
    cargo_bin_cmd!("git-forest")
        .args(["new", "test-feature"])
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_rm_recognized() {
    cargo_bin_cmd!("git-forest")
        .args(["rm", "test-feature"])
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn ls_without_config_shows_init_hint() {
    cargo_bin_cmd!("git-forest")
        .arg("ls")
        .assert()
        .failure()
        .stderr(predicates::str::contains("git forest init"));
}

#[test]
fn status_without_config_shows_init_hint() {
    cargo_bin_cmd!("git-forest")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("git forest init"));
}

#[test]
fn exec_without_config_shows_init_hint() {
    cargo_bin_cmd!("git-forest")
        .args(["exec", "test-forest", "--", "echo", "hello"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("git forest init"));
}

#[test]
fn no_args_shows_help() {
    cargo_bin_cmd!("git-forest").assert().failure();
}
