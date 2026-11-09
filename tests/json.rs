//! Integration tests for `--format json` (Phase 14.1): the real binary must
//! print exactly one parseable JSON document per invocation, keep the human
//! format as the default, and preserve the exit-code contract.
//!
//! Scoped allowance (this file only): `clippy::expect_used` — broken test
//! setup should panic with a message, and clippy's test exemption does not
//! cover integration-test crates.
#![allow(clippy::expect_used)]

mod common;

use std::fs;

use common::{TestProject, commit_all, init_git, stage_file, stderr, stdout, tree};

const USERS: &str = "\
import { Body, Controller, Get, Param, Post, Query } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get()
  findAll(@Query('page') page: number) {}

  @Get(':id')
  findOne(@Param('id') id: string) {}

  @Post()
  create(@Body() dto: CreateUserDto) {}
}
";

/// Parse the whole stdout as one JSON document (this is the output contract).
fn json(stdout: &str) -> serde_json::Value {
    match serde_json::from_str(stdout) {
        Ok(value) => value,
        Err(err) => panic!("stdout is not a single JSON document: {err}\n{stdout}"),
    }
}

#[test]
fn routes_json_lists_routes_with_structured_fields() {
    let project = TestProject::new();
    project.write("src/users/users.controller.ts", USERS);

    let output = project.run(&["routes", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doc = json(&stdout(&output));

    let routes = doc["routes"].as_array().expect("routes array");
    assert_eq!(routes.len(), 3);
    let first = &routes[0];
    assert_eq!(first["controller"], "UsersController");
    assert_eq!(first["handler"], "findAll");
    assert_eq!(first["method"], "GET");
    assert_eq!(first["path"], "/users");
    assert_eq!(
        first["parameters"][0],
        serde_json::json!({"kind": "query", "name": "page", "index": 0, "type_name": "number", "fields": []})
    );
    assert_eq!(first["body"], serde_json::json!({"shape": "none"}));
    assert_eq!(first["source"]["file"], "src/users/users.controller.ts");
    assert_eq!(first["source"]["line"], 5);
    assert_eq!(first["source"]["column"], 3);

    let create = routes
        .iter()
        .find(|route| route["handler"] == "create")
        .expect("create route");
    assert_eq!(create["body"], serde_json::json!({"shape": "unknown"}));

    assert_eq!(doc["diagnostics"], serde_json::json!([]));
    assert_eq!(doc["complete"], serde_json::json!(true));
}

#[test]
fn routes_json_on_an_empty_project_is_a_valid_empty_document() {
    let project = TestProject::new();
    project.write("src/keep.ts", "export const keep = true;");
    let output = project.run(&["routes", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doc = json(&stdout(&output));
    assert_eq!(doc["routes"], serde_json::json!([]));
    assert_eq!(doc["complete"], serde_json::json!(true));
}

#[test]
fn discover_json_lists_candidate_files() {
    let project = TestProject::new();
    project.write("src/users/users.controller.ts", USERS);
    project.write("src/other.ts", "export const x = 1;");

    let output = project.run(&["discover", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doc = json(&stdout(&output));
    assert_eq!(
        doc["files"],
        serde_json::json!(["src/users/users.controller.ts"])
    );
    assert_eq!(doc["complete"], serde_json::json!(true));
    assert!(doc["directories_read"].is_u64());
}

#[test]
fn json_output_is_deterministic_across_runs() {
    let project = TestProject::new();
    project.write("src/users/users.controller.ts", USERS);
    let first = stdout(&project.run(&["routes", "--format", "json"]));
    let second = stdout(&project.run(&["routes", "--format", "json"]));
    assert_eq!(first, second);
}

#[test]
fn sync_json_reports_created_requests_and_the_dry_run_predicts_them() {
    let project = TestProject::new();
    project.write("src/users/users.controller.ts", USERS);

    let dry = project.run(&["sync", "--dry-run", "--format", "json"]);
    assert_eq!(dry.status.code(), Some(0), "{}", stderr(&dry));
    let doc = json(&stdout(&dry));
    assert_eq!(doc["dry_run"], serde_json::json!(true));
    assert_eq!(doc["applied"], serde_json::json!(false));
    assert_eq!(
        doc["created"],
        serde_json::json!([
            "bruno/Users/create.bru",
            "bruno/Users/find-all.bru",
            "bruno/Users/find-one.bru"
        ])
    );
    assert_eq!(doc["complete"], serde_json::json!(true));
    assert!(
        !project.root().join("bruno").exists(),
        "a dry run must not create the collection"
    );

    let real = project.run(&["sync", "--format", "json"]);
    assert_eq!(real.status.code(), Some(0), "{}", stderr(&real));
    let doc = json(&stdout(&real));
    assert_eq!(doc["dry_run"], serde_json::json!(false));
    assert_eq!(doc["applied"], serde_json::json!(true));
    assert_eq!(doc["created"].as_array().map(Vec::len), Some(3));

    let before = tree(&project.root().join("bruno"));
    let again = project.run(&["sync", "--format", "json"]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    let doc = json(&stdout(&again));
    assert_eq!(doc["unchanged"].as_array().map(Vec::len), Some(3));
    assert_eq!(doc["created"], serde_json::json!([]));
    assert_eq!(tree(&project.root().join("bruno")), before);
}

#[test]
fn sync_json_reports_conflicts_and_incompleteness() {
    let project = TestProject::new();
    project.write("src/users/users.controller.ts", USERS);
    project.run(&["sync"]);

    // The user edits a generated request, then the route changes.
    let path = project.root().join("bruno/Users/find-all.bru");
    let edited = fs::read_to_string(&path)
        .expect("generated request")
        .replace("auth: inherit", "auth: bearer");
    fs::write(&path, &edited).expect("edit request");
    let changed = USERS.replace(
        "findAll(@Query('page') page: number) {}",
        "findAll(@Query('page') page: number, @Query('q') q: string) {}",
    );
    project.write_over("src/users/users.controller.ts", &changed);

    let output = project.run(&["sync", "--format", "json"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let doc = json(&stdout(&output));
    assert_eq!(doc["complete"], serde_json::json!(false));
    let conflicts = doc["conflicts"].as_array().expect("conflicts array");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"], "bruno/Users/find-all.bru");
    assert_eq!(conflicts[0]["reason"], "generated_file_modified");
    assert!(!conflicts[0]["description"].as_str().unwrap().is_empty());
    assert!(!conflicts[0]["suggestion"].as_str().unwrap().is_empty());
    // The user's edit survives: JSON output is a report, nothing more.
    assert_eq!(fs::read_to_string(&path).expect("request"), edited);
}

#[test]
fn sync_json_with_staged_changes_reports_them_structured() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");

    let output = project.run(&["sync", "--staged", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doc = json(&stdout(&output));
    let staged = doc["staged_changes"].as_array().expect("staged array");
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0]["status"], "added");
    assert_eq!(staged[0]["path"], "src/users.controller.ts");
    assert_eq!(doc["created"].as_array().map(Vec::len), Some(3));
}

#[test]
fn sync_json_without_relevant_staged_changes_is_a_valid_no_op_document() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("README.md", "# hi\n");
    stage_file(&repo, "README.md");
    commit_all(&repo, "initial");

    let output = project.run(&["sync", "--staged", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doc = json(&stdout(&output));
    assert_eq!(doc["staged_changes"], serde_json::json!([]));
    assert_eq!(doc["applied"], serde_json::json!(false));
    assert_eq!(doc["created"], serde_json::json!([]));
}

#[cfg(unix)]
#[test]
fn json_stdout_is_parseable_while_diagnostics_go_to_stderr() {
    let project = TestProject::new();
    // A nested controller that cannot be reached makes discovery incomplete.
    project.write("src/users/users.controller.ts", USERS);
    project.write("src/deep/nested/keep.ts", "");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            project.root().join("src/deep/nested"),
            fs::Permissions::from_mode(0o000),
        )
        .expect("restrict directory");
    }

    let output = project.run(&["routes", "--format", "json"]);
    let doc = json(&stdout(&output));
    assert_eq!(doc["complete"], serde_json::json!(false));
    let diagnostics = doc["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diagnostics.iter().any(|d| d["severity"] == "error"),
        "expected an error diagnostic in {diagnostics:?}"
    );
    assert!(stderr(&output).contains("error:"), "{}", stderr(&output));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            project.root().join("src/deep/nested"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("restore directory");
    }
    // Recoverable problems exit with the incomplete code, not a crash.
    assert_eq!(output.status.code(), Some(4));
}

#[test]
fn an_invalid_config_prints_no_json_at_all() {
    let project = TestProject::new();
    project.write("nestbruno.toml", "source = [\n");
    let output = project.run(&["routes", "--format", "json"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(
        stdout(&output).is_empty(),
        "stdout must not contain a partial document: {:?}",
        stdout(&output)
    );
    assert!(stderr(&output).contains("nestbruno.toml"));
}

#[test]
fn the_default_format_stays_human_readable() {
    let project = TestProject::new();
    project.write("src/users/users.controller.ts", USERS);
    let output = project.run(&["routes"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.starts_with("GET /users  UsersController.findAll"),
        "{text}"
    );
    assert!(!text.trim_start().starts_with('{'), "{text}");
}
