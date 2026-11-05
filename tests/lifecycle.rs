//! One end-to-end pass through every command in sequence, against the real
//! binary and a real git repository — the lifecycle a user actually runs:
//!
//! ```text
//! init → check → discover → routes → sync --dry-run → sync
//!      → sync --staged --dry-run → sync --staged → generate --force
//! ```
//!
//! Scoped allowance (this file only): `clippy::expect_used` — broken test
//! setup should panic with a message, and clippy's test exemption does not
//! cover integration-test crates.
#![allow(clippy::expect_used)]

mod common;

use common::{TestProject, commit_all, init_git, stage_buffer, stage_file, stderr, stdout};

const USERS: &str = "\
import { Controller, Get } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get()
  findAll() {}
}
";

#[test]
fn the_whole_command_lifecycle_holds_together() {
    let project = TestProject::new();
    let repo = init_git(&project);

    // init: creates the config, refuses to overwrite it.
    let output = project.run(&["init"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(project.root().join("nestbruno.toml").exists());
    let output = project.run(&["init"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("already exists"));

    // check: valid layout (the default source directory does not exist yet,
    // so create it).
    project.write("src/users.controller.ts", USERS);
    let output = project.run(&["check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("ok"));

    // discover: the controller candidate is listed.
    let output = project.run(&["discover"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "src/users.controller.ts\n");

    // routes: the route with its location.
    let output = project.run(&["routes"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("GET /users"),
        "{}",
        stdout(&output)
    );

    // sync --dry-run: the plan is printed, nothing is written.
    let output = project.run(&["sync", "--dry-run"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("CREATE"), "{}", stdout(&output));
    assert!(stdout(&output).contains("No files were written."));
    assert!(!project.root().join("bruno").exists());

    // sync: the request is created.
    let output = project.run(&["sync"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let request = project.root().join("bruno/Users/find-all.bru");
    assert!(request.exists());

    // sync again: nothing to do.
    let output = project.run(&["sync"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("Unchanged:   1"));

    // The user edits the request, then a staged route change arrives.
    std::fs::write(&request, "meta {\n  name: mine\n}\n").expect("edit request");
    stage_file(&repo, "src/users.controller.ts");
    commit_all(&repo, "initial");
    let changed = USERS.replace("@Get()", "@Get(':id')");
    stage_buffer(&repo, "src/users.controller.ts", changed.as_bytes());

    // sync --staged --dry-run: the conflict is predicted (exit 4, as for a
    // real run), nothing is written.
    let output = project.run(&["sync", "--staged", "--dry-run"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert!(stdout(&output).contains("CONFLICT"), "{}", stdout(&output));
    assert!(stdout(&output).contains("No files were written."));

    // sync --staged: exit 4, the user edit survives.
    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let content = std::fs::read_to_string(&request).expect("request");
    assert!(content.contains("mine"), "the user edit must survive");

    // The user resolves by removing their version; the staged state wins.
    std::fs::remove_file(&request).expect("remove request");
    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let content = std::fs::read_to_string(&request).expect("request");
    assert!(content.contains("url: {{baseUrl}}/users/:id"), "{content}");

    // The staged change is still staged (sync never touches Git): the run
    // reports it again and finds the collection already in step.
    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Staged: 1 relevant change(s)"),
        "{}",
        stdout(&output)
    );
    assert!(stdout(&output).contains("Unchanged:   1"));

    // generate works from the working tree (it is not staged-aware): bring
    // the file in line with the staged state first.
    project.write_over("src/users.controller.ts", &changed);
    let before = std::fs::read_to_string(&request).expect("request");
    let output = project.run(&["generate", "--force"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = std::fs::read_to_string(&request).expect("request");
    assert_eq!(
        before, after,
        "an unchanged generated file is not rewritten"
    );
}
