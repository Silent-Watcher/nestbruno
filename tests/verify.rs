//! Integration tests for `nestbruno verify` (Phase 14.2): the real binary
//! against real projects. The central guarantee is strict read-only
//! behavior — every test compares the filesystem before and after.
//!
//! Scoped allowance (this file only): `clippy::expect_used` — broken test
//! setup should panic with a message, and clippy's test exemption does not
//! cover integration-test crates.
#![allow(clippy::expect_used)]

mod common;

use std::fs;
use std::time::SystemTime;

use common::{TestProject, init_git, stage_file, stderr, stdout, tree};

const USERS: &str = "\
import { Controller, Get, Post } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get()
  findAll() {}

  @Post()
  create() {}
}
";

/// Sync the collection for `USERS` and return the project.
fn synced_project() -> TestProject {
    let project = TestProject::new();
    project.write("src/users/users.controller.ts", USERS);
    let output = project.run(&["sync"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    project
}

/// The full project tree (everything, not only the collection).
fn full_tree(project: &TestProject) -> Vec<(String, Vec<u8>)> {
    tree(project.root())
}

fn mtime(path: &std::path::Path) -> SystemTime {
    fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("mtime")
}

#[test]
fn verify_passes_on_a_synchronized_collection_and_touches_nothing() {
    let project = synced_project();
    let request = project.root().join("bruno/Users/find-all.bru");
    let before = full_tree(&project);
    let before_mtime = mtime(&request);

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Status: IN SYNC"), "{text}");
    assert!(text.contains("The collection was not modified."), "{text}");

    assert_eq!(full_tree(&project), before);
    assert_eq!(
        mtime(&request),
        before_mtime,
        "no rewrite, not even in place"
    );
}

#[test]
fn verify_never_creates_the_collection_directory() {
    // Regression test: verification of a project without a collection must
    // not bootstrap one — that is `sync`'s job.
    let project = TestProject::new();
    project.write("src/users/users.controller.ts", USERS);

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Status: OUT OF SYNC"), "{text}");
    assert!(text.contains("Missing:   2"), "{text}");
    assert!(
        !project.root().join("bruno").exists(),
        "verify must not create the collection directory"
    );
}

#[test]
fn verify_detects_a_missing_generated_request() {
    let project = synced_project();
    fs::remove_file(project.root().join("bruno/Users/find-all.bru")).expect("remove request");
    let before = full_tree(&project);

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Status: OUT OF SYNC"), "{text}");
    assert!(text.contains("Missing:   1"), "{text}");
    assert!(text.contains("Run `nestbruno sync` to reconcile"), "{text}");
    assert_eq!(
        full_tree(&project),
        before,
        "the deleted file stays deleted, nothing else appears"
    );
}

#[test]
fn verify_detects_an_outdated_request() {
    let project = synced_project();
    let changed = USERS
        .replace("findAll() {}", "findAll(@Query('q') q: string) {}")
        .replace(
            "import { Controller, Get, Post }",
            "import { Controller, Get, Post, Query }",
        );
    project.write_over("src/users/users.controller.ts", &changed);
    let before = full_tree(&project);

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Outdated:  1"), "{text}");
    assert_eq!(
        full_tree(&project),
        before,
        "the outdated request is reported, not rewritten"
    );
}

#[test]
fn verify_detects_a_stale_request() {
    let project = synced_project();
    let fewer = USERS.replace("  @Post()\n  create() {}\n", "");
    project.write_over("src/users/users.controller.ts", &fewer);
    let before = full_tree(&project);

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Stale:     1"), "{text}");
    assert_eq!(
        full_tree(&project),
        before,
        "the stale request is reported, not deleted"
    );
}

#[test]
fn verify_reports_user_modifications_as_conflicts_without_touching_them() {
    let project = synced_project();
    let path = project.root().join("bruno/Users/find-all.bru");
    let edited = fs::read_to_string(&path)
        .expect("generated request")
        .replace("auth: inherit", "auth: bearer");
    fs::write(&path, &edited).expect("edit request");
    let changed = USERS
        .replace("findAll() {}", "findAll(@Query('q') q: string) {}")
        .replace(
            "import { Controller, Get, Post }",
            "import { Controller, Get, Post, Query }",
        );
    project.write_over("src/users/users.controller.ts", &changed);
    let before = full_tree(&project);

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Status: BLOCKED"), "{text}");
    assert!(text.contains("Conflicts: 1"), "{text}");
    assert!(text.contains("CONFLICT"), "{text}");
    assert_eq!(full_tree(&project), before);
    assert_eq!(fs::read_to_string(&path).expect("request"), edited);
}

#[test]
fn verify_reports_critical_conflicts() {
    let project = synced_project();
    // A clone of a generated request (what Bruno's "clone" produces) makes
    // two files claim the same route identity: a critical conflict.
    let original = fs::read_to_string(project.root().join("bruno/Users/find-all.bru"))
        .expect("generated request");
    fs::write(project.root().join("bruno/Users/copy.bru"), &original).expect("clone request");
    let changed = USERS
        .replace("findAll() {}", "findAll(@Query('q') q: string) {}")
        .replace(
            "import { Controller, Get, Post }",
            "import { Controller, Get, Post, Query }",
        );
    project.write_over("src/users/users.controller.ts", &changed);
    let before = full_tree(&project);

    let output = project.run(&["verify", "--format", "json"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&output)).expect("one JSON document");
    assert_eq!(doc["status"], "blocked");
    let conflicts = doc["conflicts"].as_array().expect("conflicts");
    assert!(
        conflicts
            .iter()
            .any(|c| c["reason"] == "duplicate_bruno_identity"),
        "{conflicts:?}"
    );
    assert_eq!(full_tree(&project), before);
}

#[test]
fn verify_on_an_empty_project_is_in_sync_and_creates_nothing() {
    let project = TestProject::new();
    project.write("src/keep.ts", "export const keep = true;");

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Status: IN SYNC"), "{text}");
    assert!(
        !project.root().join("bruno").exists(),
        "verify must not create an empty collection"
    );
}

#[test]
fn verify_never_touches_unknown_files() {
    let project = synced_project();
    fs::create_dir_all(project.root().join("bruno/environments")).expect("env dir");
    fs::write(
        project.root().join("bruno/environments/local.bru"),
        "vars {\n  a: 1\n}\n",
    )
    .expect("env file");
    fs::write(
        project.root().join("bruno/mine.bru"),
        "meta {\n  name: mine\n}\n",
    )
    .expect("own file");
    let before = full_tree(&project);

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Preserved: 2"), "{text}");
    assert_eq!(full_tree(&project), before);
}

#[cfg(unix)]
#[test]
fn verify_is_safe_when_analysis_is_incomplete() {
    use std::os::unix::fs::PermissionsExt;

    let project = synced_project();
    project.write("src/deep/nested/keep.ts", "");
    fs::set_permissions(
        project.root().join("src/deep/nested"),
        fs::Permissions::from_mode(0o000),
    )
    .expect("restrict directory");
    // The restricted directory itself cannot be walked, so snapshot only
    // what is readable: the collection.
    let before = tree(&project.root().join("bruno"));

    let output = project.run(&["verify"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Status: INCOMPLETE"), "{text}");
    assert!(stderr(&output).contains("error:"), "{}", stderr(&output));
    assert_eq!(
        tree(&project.root().join("bruno")),
        before,
        "nothing is modified on an incomplete analysis"
    );
    assert_eq!(
        fs::symlink_metadata(project.root().join("src/deep/nested"))
            .expect("restricted dir")
            .permissions()
            .mode()
            & 0o777,
        0,
        "verify must not have touched even the unreadable directory"
    );

    fs::set_permissions(
        project.root().join("src/deep/nested"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("restore directory");
}

#[test]
fn verify_json_reports_status_summary_and_changes() {
    let project = synced_project();
    fs::remove_file(project.root().join("bruno/Users/find-all.bru")).expect("remove request");

    let output = project.run(&["verify", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&output)).expect("one JSON document");
    assert_eq!(doc["status"], "out_of_sync");
    assert_eq!(doc["complete"], true);
    assert_eq!(
        doc["summary"],
        serde_json::json!({
            "missing": 1, "outdated": 0, "stale": 0,
            "unchanged": 1, "conflicts": 0, "preserved": 0
        })
    );
    assert_eq!(
        doc["changes"]["created"],
        serde_json::json!(["bruno/Users/find-all.bru"])
    );
    assert_eq!(doc["changes"]["updated"], serde_json::json!([]));
    assert_eq!(doc["changes"]["deleted"], serde_json::json!([]));
    assert_eq!(doc["diagnostics"], serde_json::json!([]));
}

#[test]
fn verify_json_is_deterministic() {
    let project = synced_project();
    let first = stdout(&project.run(&["verify", "--format", "json"]));
    let second = stdout(&project.run(&["verify", "--format", "json"]));
    assert_eq!(first, second);
}

#[test]
fn staged_verify_detects_drift_from_the_index() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");

    let output = project.run(&["verify", "--staged"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Status: OUT OF SYNC"), "{text}");
    assert!(text.contains("Missing:   2"), "{text}");
    assert!(
        !project.root().join("bruno").exists(),
        "staged verification must not create the collection"
    );
}

#[test]
fn staged_verify_without_relevant_changes_is_a_no_op() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("README.md", "# hi\n");
    stage_file(&repo, "README.md");

    let output = project.run(&["verify", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("nothing to verify"),
        "{}",
        stdout(&output)
    );
    assert!(!project.root().join("bruno").exists());
}

#[test]
fn verify_with_an_invalid_config_prints_no_json() {
    let project = TestProject::new();
    project.write("nestbruno.toml", "source = [\n");
    let output = project.run(&["verify", "--format", "json"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("nestbruno.toml"));
}
