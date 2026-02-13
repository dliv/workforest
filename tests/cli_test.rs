use assert_cmd::Command;

#[test]
fn help_exits_zero() {
    Command::cargo_bin("git-forest")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
}

#[test]
fn subcommand_init_recognized() {
    Command::cargo_bin("git-forest")
        .unwrap()
        .arg("init")
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_new_recognized() {
    Command::cargo_bin("git-forest")
        .unwrap()
        .args(["new", "test-feature"])
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_rm_recognized() {
    Command::cargo_bin("git-forest")
        .unwrap()
        .args(["rm", "test-feature"])
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_ls_recognized() {
    Command::cargo_bin("git-forest")
        .unwrap()
        .arg("ls")
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_status_recognized() {
    Command::cargo_bin("git-forest")
        .unwrap()
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn subcommand_exec_recognized() {
    Command::cargo_bin("git-forest")
        .unwrap()
        .args(["exec", "test-forest", "--", "echo", "hello"])
        .assert()
        .success()
        .stderr(predicates::str::contains("not yet implemented"));
}

#[test]
fn no_args_shows_help() {
    Command::cargo_bin("git-forest")
        .unwrap()
        .assert()
        .failure();
}
