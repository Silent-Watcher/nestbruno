mod common;

use std::fs;
use std::process::Command;

use common::{TestProject, stderr, stdout};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nestbruno"))
}

#[test]
fn no_arguments_prints_usage_and_fails_with_code_2() {
    let output = bin().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("Usage"));
}

#[test]
fn help_lists_all_commands() {
    let output = bin().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = stdout(&output);
    for command in ["init", "check", "sync", "watch"] {
        assert!(help.contains(command), "help should mention {command}");
    }
}

#[test]
fn unknown_command_fails_with_code_2() {
    let output = bin().arg("frobnicate").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn init_creates_a_loadable_config() {
    let project = TestProject::new();
    let output = project.run(&["init"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(project.root().join("nestbruno.toml").is_file());

    project.write("src/keep.txt", "");
    let check = project.run(&["check"]);
    assert!(check.status.success(), "{}", stderr(&check));
}

#[test]
fn init_never_overwrites_an_existing_config() {
    let project = TestProject::new();
    project.write("nestbruno.toml", "source = \"lib\"\n");
    let output = project.run(&["init"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("already exists"));
    assert_eq!(
        fs::read_to_string(project.root().join("nestbruno.toml")).unwrap(),
        "source = \"lib\"\n"
    );
}

#[test]
fn check_works_without_a_config_file() {
    let project = TestProject::new();
    project.write("src/a.ts", "");
    let output = project.run(&["check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("config:  (defaults)"), "{text}");
    assert!(text.contains("source:  src"), "{text}");
    assert!(text.trim_end().ends_with("ok"), "{text}");
}

#[test]
fn check_reports_a_custom_configuration() {
    let project = TestProject::new();
    project.write(
        "nestbruno.toml",
        "source = \"apps/api\"\nbruno = \"docs/bruno\"\ninclude = [\"**/*.ctrl.ts\"]\n",
    );
    project.write("apps/api/main.ts", "");
    let output = project.run(&["check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("source:  apps/api"), "{text}");
    assert!(text.contains("include: **/*.ctrl.ts"), "{text}");
}

#[test]
fn check_with_invalid_toml_fails_with_config_exit_code() {
    let project = TestProject::new();
    project.write("nestbruno.toml", "source = [\n");
    let output = project.run(&["check"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains("nestbruno.toml"));
}

#[test]
fn check_rejects_path_traversal_in_configuration() {
    let project = TestProject::new();
    project.write("nestbruno.toml", "bruno = \"../outside\"\n");
    let output = project.run(&["check"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains(".."));
}

#[test]
fn check_with_missing_source_directory_explains_how_to_fix_it() {
    let project = TestProject::new();
    let output = project.run(&["check"]);
    assert_eq!(output.status.code(), Some(3));
    let message = stderr(&output);
    assert!(message.contains("does not exist"), "{message}");
    assert!(message.contains("`source`"), "{message}");
}

#[test]
fn check_with_missing_project_directory_fails() {
    let project = TestProject::new();
    let missing = project.root().join("nope");
    let output = bin().arg("-C").arg(&missing).arg("check").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("nope"));
}

#[test]
fn sync_and_watch_are_reported_as_not_implemented() {
    let project = TestProject::new();
    for command in ["sync", "watch"] {
        let output = project.run(&[command]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert!(stderr(&output).contains("not implemented"), "{command}");
    }
}

#[test]
fn verbose_mentions_defaults_when_no_config_exists() {
    let project = TestProject::new();
    project.write("src/a.ts", "");
    let quiet = project.run(&["check"]);
    assert!(!stderr(&quiet).contains("using defaults"));
    let verbose = project.run(&["check", "--verbose"]);
    assert!(stderr(&verbose).contains("using defaults"));
}
