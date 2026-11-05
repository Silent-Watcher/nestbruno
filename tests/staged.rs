//! End-to-end tests for `nestbruno sync --staged` against the real binary and
//! real git repositories.
//!
//! Every repository is created in a throwaway temp directory through libgit2;
//! no test touches the developer's own git configuration or repositories, and
//! no test commits to anything outside its temp dir. Staging is done through
//! the git2 API (blobs written directly into the index), so a test can stage
//! content that differs from the working tree.
//!
//! Scoped allowance (this file only): `clippy::expect_used` — broken test
//! setup should panic with a message, and clippy's test exemption does not
//! cover integration-test crates.
#![allow(clippy::expect_used)]

mod common;

use common::{TestProject, stderr, stdout, tree};
use git2::{IndexEntry, IndexTime, Repository, Signature};

/// Stage `rel` (worktree-relative) with `content` without touching the
/// working tree.
fn stage_buffer(repo: &Repository, rel: &str, content: &[u8]) {
    let id = repo.blob(content).expect("write blob");
    let mut index = repo.index().expect("read index");
    let entry = IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: 0o100_644,
        uid: 0,
        gid: 0,
        file_size: u32::try_from(content.len()).expect("small blob"),
        id,
        flags: 0,
        flags_extended: 0,
        path: rel.as_bytes().to_vec(),
    };
    index.add_frombuffer(&entry, content).expect("stage buffer");
    index.write().expect("write index");
}

/// Stage the working-tree version of `rel`.
fn stage_file(repo: &Repository, rel: &str) {
    let mut index = repo.index().expect("read index");
    index.add_path(std::path::Path::new(rel)).expect("add path");
    index.write().expect("write index");
}

fn unstage(repo: &Repository, rel: &str) {
    let mut index = repo.index().expect("read index");
    index
        .remove_path(std::path::Path::new(rel))
        .expect("remove path");
    index.write().expect("write index");
}

/// Commit the whole index so HEAD exists.
fn commit_all(repo: &Repository, message: &str) {
    let mut index = repo.index().expect("read index");
    index.write().expect("write index");
    let tree_id = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_id).expect("find tree");
    let signature = Signature::now("test", "test@example.invalid").expect("signature");
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )
    .expect("commit");
}

fn init_git(project: &TestProject) -> Repository {
    Repository::init(project.root()).expect("init repo")
}

const USERS: &str = "\
import { Controller, Get } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get()
  findAll() {}
}
";

const AUTH: &str = "\
import { Controller, Post } from '@nestjs/common';

@Controller('auth')
export class AuthController {
  @Post('login')
  login() {}
}
";

#[test]
fn a_missing_repository_is_a_clear_error() {
    let project = TestProject::new();
    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("no git repository"), "{message}");
}

#[test]
fn no_relevant_staged_changes_is_a_clean_no_op() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("README.md", "# hi\n");
    stage_file(&repo, "README.md");

    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "No relevant staged NestJS changes detected.\n"
    );
    assert!(
        !project.root().join("bruno").exists(),
        "the collection directory must not be created"
    );
}

#[test]
fn a_staged_controller_creates_the_request() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");

    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("CREATE"), "{}", stdout(&output));
    let request = project.root().join("bruno/Users/find-all.bru");
    let content = std::fs::read_to_string(&request).expect("request exists");
    assert!(content.contains("url: {{baseUrl}}/users"), "{content}");
}

#[test]
fn the_staged_version_is_analyzed_not_the_working_tree() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");
    commit_all(&repo, "initial");

    // Working tree: 'accounts'. Staged: 'clients'. The staged one wins.
    let accounts = USERS.replace("'users'", "'accounts'");
    project.write_over("src/users.controller.ts", &accounts);
    let clients = USERS.replace("'users'", "'clients'");
    stage_buffer(&repo, "src/users.controller.ts", clients.as_bytes());

    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let request = std::fs::read_to_string(project.root().join("bruno/Users/find-all.bru"))
        .expect("request exists");
    assert!(request.contains("{{baseUrl}}/clients"), "{request}");
    assert!(!request.contains("{{baseUrl}}/accounts"), "{request}");
}

#[test]
fn unstaged_changes_are_ignored() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");
    commit_all(&repo, "initial");

    // First run: the index matches HEAD; nothing staged.
    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("No relevant staged"));
    assert!(!project.root().join("bruno").exists());

    // An unstaged edit alone changes nothing.
    project.write_over(
        "src/users.controller.ts",
        &USERS.replace("'users'", "'accounts'"),
    );
    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("No relevant staged"));
    assert!(!project.root().join("bruno").exists());
}

#[test]
fn a_staged_deletion_deletes_the_unmodified_request() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");
    commit_all(&repo, "initial");

    // The request exists (created by a plain sync of the working tree).
    let output = project.run(&["sync"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let request = project.root().join("bruno/Users/find-all.bru");
    assert!(request.exists());

    unstage(&repo, "src/users.controller.ts");
    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("DELETE"), "{}", stdout(&output));
    assert!(!request.exists());
    assert!(
        !project.root().join("bruno/Users").exists(),
        "emptied folders are removed"
    );
}

#[test]
fn a_staged_rename_updates_the_request_in_place() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/old.controller.ts", USERS);
    stage_file(&repo, "src/old.controller.ts");
    commit_all(&repo, "initial");

    let output = project.run(&["sync"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let request = project.root().join("bruno/Users/find-all.bru");
    assert!(request.exists());

    // Stage the rename (same content, new path): the route identity changes
    // (it includes the file), but the request is updated in place instead of
    // being deleted and re-created.
    unstage(&repo, "src/old.controller.ts");
    project.write("src/new.controller.ts", USERS);
    project.write_over("src/old.controller.ts", USERS);
    stage_file(&repo, "src/new.controller.ts");

    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("renamed src/new.controller.ts"), "{text}");
    assert!(text.contains("UPDATE"), "{text}");
    assert!(!text.contains("CREATE"), "{text}");
    assert!(!text.contains("DELETE"), "{text}");
    assert!(request.exists());
}

#[test]
fn a_user_modified_request_conflicts_instead_of_being_overwritten() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");
    project.run(&["sync", "--staged"]);

    let request = project.root().join("bruno/Users/find-all.bru");
    std::fs::write(&request, "meta {\n  name: my own version\n}\n").expect("edit request");

    // Stage a route change; the edited request must not be touched.
    let changed = USERS.replace("@Get()", "@Get(':id')");
    stage_buffer(&repo, "src/users.controller.ts", changed.as_bytes());

    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("CONFLICT"), "{text}");
    let content = std::fs::read_to_string(&request).expect("request");
    assert!(content.contains("my own version"), "user edit destroyed");
}

#[test]
fn dry_run_prints_the_plan_and_writes_nothing() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");

    let output = project.run(&["sync", "--staged", "--dry-run"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("CREATE"), "{text}");
    assert!(text.contains("No files were written."), "{text}");
    assert!(
        !project.root().join("bruno").exists(),
        "dry run must not create the collection"
    );

    // The real run then applies exactly the planned change.
    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(project.root().join("bruno/Users/find-all.bru").exists());
}

#[test]
fn a_second_run_makes_no_changes() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    stage_file(&repo, "src/users.controller.ts");
    project.run(&["sync", "--staged"]);

    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Unchanged:   1"), "{text}");
    assert!(!text.contains("CREATE"), "{text}");
}

#[test]
fn unicode_and_spaces_in_staged_paths_work() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/کاربران/my users.controller.ts", USERS);
    stage_file(&repo, "src/کاربران/my users.controller.ts");

    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("CREATE"), "{text}");
    assert!(project.root().join("bruno/Users/find-all.bru").exists());
}

#[test]
fn a_project_nested_in_the_worktree_is_supported() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("app/src/users.controller.ts", USERS);
    stage_file(&repo, "app/src/users.controller.ts");

    // Run against the nested project directory (which has no repository of
    // its own; the enclosing worktree is used, and index paths are mapped).
    let app = project.root().join("app");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nestbruno"))
        .arg("-C")
        .arg(&app)
        .args(["sync", "--staged"])
        .output()
        .expect("run nestbruno");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("added src/users.controller.ts"), "{text}");
    assert!(app.join("bruno/Users/find-all.bru").exists());
}

/// The core invariant: staged synchronization equals a clean synchronization
/// of the project state that the index represents.
#[test]
fn staged_sync_equals_a_fresh_sync_of_the_staged_state() {
    let make_source = |name: &str, prefix: &str| -> String {
        format!(
            "\
import {{ Controller, Get }} from '@nestjs/common';

@Controller('{prefix}')
export class {name}Controller {{
  @Get()
  findAll() {{}}
}}
"
        )
    };

    // Project A: synchronize from the index via `sync --staged`.
    let a = TestProject::new();
    let repo_a = init_git(&a);
    a.write("src/users.controller.ts", &make_source("Users", "users"));
    a.write("src/auth.controller.ts", AUTH);
    stage_file(&repo_a, "src/users.controller.ts");
    stage_file(&repo_a, "src/auth.controller.ts");
    commit_all(&repo_a, "initial");
    // Stage: modify users, delete auth, add billing.
    let changed = make_source("Users", "clients");
    stage_buffer(&repo_a, "src/users.controller.ts", changed.as_bytes());
    unstage(&repo_a, "src/auth.controller.ts");
    let billing = make_source("Billing", "billing");
    a.write("src/billing.controller.ts", &billing);
    stage_file(&repo_a, "src/billing.controller.ts");

    let output = a.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    // Project B: the same state materialized on disk, plain `sync`.
    let b = TestProject::new();
    b.write("src/users.controller.ts", &changed);
    b.write("src/billing.controller.ts", &billing);
    let output = b.run(&["sync"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    // The collections must be byte-identical (the stamp includes the source
    // file path, which is identical in both projects).
    let collection_a = a.root().join("bruno");
    let collection_b = b.root().join("bruno");
    assert_eq!(
        tree(&collection_a),
        tree(&collection_b),
        "staged sync and a fresh sync of the staged state must agree"
    );
}

#[test]
fn unchanged_paths_of_an_incomplete_run_are_still_synced_and_nothing_is_deleted() {
    let project = TestProject::new();
    let repo = init_git(&project);
    project.write("src/users.controller.ts", USERS);
    project.write("src/auth.controller.ts", AUTH);
    stage_file(&repo, "src/users.controller.ts");
    stage_file(&repo, "src/auth.controller.ts");
    project.run(&["sync", "--staged"]);

    // Stage a change to one file; stage a second file that cannot be parsed.
    let changed = USERS.replace("@Get()", "@Get(':id')");
    stage_buffer(&repo, "src/users.controller.ts", changed.as_bytes());
    stage_buffer(
        &repo,
        "src/auth.controller.ts",
        b"@Controller('auth'\n".as_slice(),
    );

    let output = project.run(&["sync", "--staged"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let text = stdout(&output);
    // The valid file is still updated...
    assert!(text.contains("UPDATE"), "{text}");
    // ...and the unparsable one is reported without deleting anything.
    assert!(text.contains("Deleted:     0"), "{text}");
    let message = stderr(&output);
    assert!(
        message.contains("unexpected") || message.contains("error"),
        "{message}"
    );
}
