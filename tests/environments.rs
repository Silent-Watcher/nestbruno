//! Integration tests for `nestbruno generate --environments` (Phase 16).
//! Security-first: generated environment files contain empty placeholders
//! only, existing environment files are never modified, and unsafe names
//! write nothing anywhere.
//!
//! Scoped allowance (this file only): `clippy::expect_used` — broken test
//! setup should panic with a message, and clippy's test exemption does not
//! cover integration-test crates.
#![allow(clippy::expect_used)]

mod common;

use std::fs;

use common::{TestProject, stderr, stdout, tree};

const USERS: &str = "\
import { Controller, Get } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get()
  findAll() {}
}
";

fn project() -> TestProject {
    let p = TestProject::new();
    p.write("src/users/users.controller.ts", USERS);
    p
}

#[test]
fn generate_creates_an_environment_with_an_empty_placeholder() {
    let p = project();
    let output = p.run(&["generate", "--environments"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("Environments (development): 1 created"),
        "{text}"
    );
    let env = p.root().join("bruno/environments/development.bru");
    assert_eq!(
        fs::read_to_string(&env).expect("environment file"),
        "vars {\n  baseUrl: \n}\n",
        "the placeholder is empty; no value is ever invented"
    );
}

#[test]
fn multiple_environment_names_share_one_registry() {
    let p = project();
    let output = p.run(&["generate", "--environments", "dev,staging,dev"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("Environments (dev, staging): 2 created"),
        "{text}"
    );
    let dev = fs::read_to_string(p.root().join("bruno/environments/dev.bru")).expect("dev");
    let staging =
        fs::read_to_string(p.root().join("bruno/environments/staging.bru")).expect("staging");
    assert_eq!(dev, staging, "structurally identical, one shared registry");
    assert_eq!(dev, "vars {\n  baseUrl: \n}\n");
}

#[test]
fn generation_is_deterministic() {
    // Two fresh projects produce byte-identical output and files.
    let first = project();
    let first_output = stdout(&first.run(&["generate", "--environments", "prod"]));
    let second = project();
    let second_output = stdout(&second.run(&["generate", "--environments", "prod"]));
    assert_eq!(first_output, second_output);
    assert_eq!(
        fs::read_to_string(first.root().join("bruno/environments/prod.bru")).expect("env"),
        fs::read_to_string(second.root().join("bruno/environments/prod.bru")).expect("env")
    );
    assert_eq!(
        fs::read_to_string(first.root().join("bruno/environments/prod.bru")).expect("env"),
        "vars {\n  baseUrl: \n}\n"
    );
}

#[test]
fn an_existing_environment_file_is_never_modified_even_with_force() {
    let p = project();
    p.run(&["generate", "--environments"]);
    let path = p.root().join("bruno/environments/development.bru");
    // The user fills in a real value and adds their own variable.
    let user_content = "vars {\n  baseUrl: http://localhost:3000\n  my_own: keep\n}\n";
    fs::write(&path, user_content).expect("edit environment");
    let before = tree(&p.root().join("bruno"));
    let before_mtime = fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");

    let output = p.run(&["generate", "--environments", "--force"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("1 already exist(s) (not modified)"), "{text}");
    assert_eq!(
        fs::read_to_string(&path).expect("environment"),
        user_content
    );
    assert_eq!(tree(&p.root().join("bruno")), before);
    assert_eq!(
        fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime"),
        before_mtime,
        "not even rewritten in place"
    );
}

#[test]
fn a_literal_base_url_needs_no_environment_files() {
    let p = project();
    p.write("nestbruno.toml", "base_url = \"http://localhost:3000\"\n");
    let output = p.run(&["generate", "--environments"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        !text.contains("Environments"),
        "no variables, no environment files: {text}"
    );
    assert!(!p.root().join("bruno/environments").exists());
}

#[test]
fn several_variables_are_sorted_into_one_file() {
    let p = project();
    p.write(
        "nestbruno.toml",
        "base_url = \"{{apiVersion}}.{{baseUrl}}\"\n",
    );
    let output = p.run(&["generate", "--environments"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let env = fs::read_to_string(p.root().join("bruno/environments/development.bru"))
        .expect("environment file");
    assert_eq!(env, "vars {\n  apiVersion: \n  baseUrl: \n}\n");
}

#[test]
fn a_source_expression_reference_is_reported_not_generated() {
    let p = project();
    // `{{ process.env.API_URL }}` is a code expression, not a Bruno
    // variable: names only in the diagnostic, no file, no value anywhere.
    p.write(
        "nestbruno.toml",
        "base_url = \"{{ process.env.API_URL }}\"\n",
    );
    let output = p.run(&["generate", "--environments"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("process.env.API_URL"), "{message}");
    assert!(
        !message.contains("http"),
        "no values may appear in diagnostics: {message}"
    );
    assert!(!p.root().join("bruno/environments").exists());
}

#[test]
fn an_unsafe_environment_name_writes_nothing_anywhere() {
    let p = project();
    let output = p.run(&["generate", "--environments", "../evil"]);
    // A rejected name is a per-file failure, consistent with generate's
    // error policy: the run finishes, the exit code marks it incomplete.
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("not a safe file name"), "{message}");
    assert!(!p.root().join("bruno/environments").exists());
    assert!(!p.root().join("evil.bru").exists());
    assert!(!p.root().parent().unwrap().join("evil.bru").exists());
}

#[test]
fn an_environment_file_is_user_owned_for_every_later_command() {
    let p = project();
    p.run(&["generate", "--environments"]);
    let path = p.root().join("bruno/environments/development.bru");
    let user_content = "vars {\n  baseUrl: http://localhost:3000\n}\n";
    fs::write(&path, user_content).expect("fill in values");
    // sync, sync --dry-run and verify must leave it exactly as it is.
    for args in [
        vec!["sync"],
        vec!["sync", "--dry-run"],
        vec!["verify"],
        vec!["generate", "--force"],
    ] {
        let output = p.run(&args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}: {}",
            args.join(" "),
            stderr(&output)
        );
    }
    let mut after = tree(p.root());
    // Requests may legitimately have been created by the first sync.
    after.retain(|(path, _)| !path.starts_with("bruno/Users/"));
    assert_eq!(
        fs::read_to_string(&path).expect("environment"),
        user_content,
        "the user's values survive sync, dry-run, verify and generate --force"
    );
}

#[test]
fn generate_without_the_flag_stays_backward_compatible() {
    let p = project();
    let output = p.run(&["generate"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!p.root().join("bruno/environments").exists());
    assert!(!stdout(&output).contains("Environments"));
}

#[test]
fn sync_reports_the_environment_file_as_preserved() {
    let p = project();
    p.run(&["generate", "--environments"]);
    let path = p.root().join("bruno/environments/development.bru");
    let user_content = "vars {\n  baseUrl: http://localhost:3000\n}\n";
    fs::write(&path, user_content).expect("fill in values");

    let output = p.run(&["sync", "--verbose"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("PRESERVE  bruno/environments/development.bru"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        fs::read_to_string(&path).expect("environment"),
        user_content
    );
}
