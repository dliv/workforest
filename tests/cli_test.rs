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
fn subcommand_ls_recognized() {
    cargo_bin_cmd!("git-forest")
        .arg("ls")
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_status_recognized() {
    cargo_bin_cmd!("git-forest")
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_exec_recognized() {
    cargo_bin_cmd!("git-forest")
        .args(["exec", "test-forest", "--", "echo", "hello"])
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn no_args_shows_help() {
    cargo_bin_cmd!("git-forest").assert().failure();
}
