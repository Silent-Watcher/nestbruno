//! Integration tests for `nestbruno completions` (Phase 14.3): valid,
//! deterministic scripts written exclusively to stdout.
//!
//! Scoped allowance (this file only): `clippy::expect_used` — broken test
//! setup should panic with a message, and clippy's test exemption does not
//! cover integration-test crates.
#![allow(clippy::expect_used)]

mod common;

use common::{TestProject, stderr, stdout};

const SHELLS: [&str; 4] = ["bash", "zsh", "fish", "powershell"];

#[test]
fn every_supported_shell_gets_a_nonempty_script_naming_the_tool() {
    let project = TestProject::new();
    for shell in SHELLS {
        let output = project.run(&["completions", shell]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let script = stdout(&output);
        assert!(!script.trim().is_empty(), "{shell}: empty script");
        assert!(script.contains("nestbruno"), "{shell}: tool name missing");
        assert!(
            stderr(&output).is_empty(),
            "{shell}: stdout must be the only output: {:?}",
            stderr(&output)
        );
    }
}

#[test]
fn scripts_mention_the_subcommands_and_options() {
    let project = TestProject::new();
    let bash = stdout(&project.run(&["completions", "bash"]));
    for name in [
        "sync",
        "verify",
        "completions",
        "--format",
        "--dry-run",
        "--staged",
    ] {
        assert!(bash.contains(name), "bash script should mention {name}");
    }
    let zsh = stdout(&project.run(&["completions", "zsh"]));
    for name in ["verify", "--format", "--staged"] {
        assert!(zsh.contains(name), "zsh script should mention {name}");
    }
    let fish = stdout(&project.run(&["completions", "fish"]));
    for name in ["verify", "format"] {
        assert!(fish.contains(name), "fish script should mention {name}");
    }
}

#[test]
fn generation_is_stable_across_runs() {
    let project = TestProject::new();
    for shell in SHELLS {
        let first = stdout(&project.run(&["completions", shell]));
        let second = stdout(&project.run(&["completions", shell]));
        assert_eq!(first, second, "{shell}: generation must be deterministic");
    }
}

#[test]
fn an_unknown_shell_is_a_usage_error() {
    let project = TestProject::new();
    let output = project.run(&["completions", "tcsh"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("invalid value"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_shell_argument_is_required() {
    let project = TestProject::new();
    let output = project.run(&["completions"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn help_lists_the_completions_command() {
    let project = TestProject::new();
    let output = project.run(&["--help"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("completions"));
}
