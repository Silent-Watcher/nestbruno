//! End-to-end tests of watch mode: the real binary and the real OS file
//! watcher. Nothing here sleeps for a fixed time: the tests wait for output
//! lines, and prove "nothing happened" with a *sentinel*: a later change whose
//! report must be the next one, because output is ordered.
#![cfg(unix)]
// Scoped allowance (this file only): the harness panics with a message when the
// process under test cannot be started or controlled, the right failure mode
// for test infrastructure; clippy's test exemption does not cover helpers.
#![allow(clippy::expect_used)]

mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use common::{TestProject, tree};

const TIMEOUT: Duration = Duration::from_secs(30);

fn controller(name: &str, route: &str, handler: &str) -> String {
    format!(
        "@Controller('{name}') export class {name}Controller {{ @Get('{route}') {handler}() {{}} @Post() create{name}() {{}} }}"
    )
}

struct Watching {
    child: Child,
    out: Receiver<String>,
    err: Receiver<String>,
    /// Everything read from stdout so far.
    seen: Vec<String>,
    sentinels: usize,
}

fn pump(stream: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Watching {
    fn start(project: &TestProject) -> Self {
        Self::start_with(project, &[])
    }

    fn start_with(project: &TestProject, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nestbruno"))
            .arg("-C")
            .arg(project.root())
            .arg("watch")
            .args(if args.contains(&"--debounce") {
                &[][..]
            } else {
                &["--debounce", "40"][..]
            })
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn nestbruno watch");
        let out = pump(child.stdout.take().expect("stdout"));
        let err = pump(child.stderr.take().expect("stderr"));
        let mut watching = Self {
            child,
            out,
            err,
            seen: Vec::new(),
            sentinels: 0,
        };
        watching.wait_for("watching", |l| l.starts_with("Watching "));
        watching
    }

    /// Read stdout until `pred` matches a line; panics on timeout or exit.
    fn wait_for(&mut self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        loop {
            match self.out.recv_timeout(TIMEOUT) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if pred(&line) {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!(
                        "timed out waiting for {what}; stdout so far:\n{}",
                        self.seen.join("\n")
                    )
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "the process exited while waiting for {what}; stdout:\n{}",
                        self.seen.join("\n")
                    )
                }
            }
        }
    }

    fn wait_changed(&mut self, file: &str) {
        let expected = format!("Changed: {file}");
        self.wait_for(&expected.clone(), move |l| l == expected);
    }

    /// Wait until the report of the next change is complete: the three count lines.
    fn counts_after(&mut self, file: &str) -> String {
        self.wait_changed(file);
        self.wait_for("routes affected", |l| l.contains("Routes affected"));
        self.wait_for("counts", |l| l.contains("Created:"))
    }

    /// Make a real change elsewhere and wait for its report. Everything printed
    /// before it is in `seen`; nothing else may have been reported in between.
    fn sentinel(&mut self, project: &TestProject) {
        self.sentinels += 1;
        let name = format!("src/zz{}.controller.ts", self.sentinels);
        project.write(
            &name,
            &controller(&format!("zz{}", self.sentinels), "s", "sentinel"),
        );
        self.wait_changed(&name);
    }

    /// Like [`Self::sentinel`] for a project whose source directory is `.`.
    fn sentinel_at(&mut self, project: &TestProject, name: &str) {
        project.write(name, &controller("zz1", "s", "sentinel"));
        self.wait_changed(name);
    }

    fn changed_lines(&self) -> Vec<&str> {
        self.seen
            .iter()
            .filter(|l| l.starts_with("Changed: "))
            .map(String::as_str)
            .collect()
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Send `signal`, wait for the process to close its output, return its status.
    fn stop_with(&mut self, signal: &str) -> std::process::ExitStatus {
        let status = Command::new("kill")
            .args(["-s", signal, &self.pid().to_string()])
            .status()
            .expect("run kill");
        assert!(status.success());
        while let Ok(line) = self.out.recv_timeout(TIMEOUT) {
            self.seen.push(line);
        }
        self.child.wait().expect("wait for the process")
    }

    fn stderr_text(&mut self) -> String {
        let mut lines = Vec::new();
        while let Ok(line) = self.err.try_recv() {
            lines.push(line);
        }
        lines.join("\n")
    }

    /// Wait for a specific stderr line.
    fn wait_stderr(&mut self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        loop {
            match self.err.recv_timeout(TIMEOUT) {
                Ok(line) if pred(&line) => return line,
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for stderr: {what}"),
            }
        }
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        // Never leak a watcher process, even when a test fails.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn bruno(p: &TestProject, rel: &str) -> String {
    common::read_text(&p.root().join("bruno").join(rel))
}

fn project() -> TestProject {
    let p = TestProject::new();
    p.write(
        "src/users/users.controller.ts",
        &controller("users", ":id", "findOne"),
    );
    p.write(
        "src/auth/auth.controller.ts",
        &controller("auth", "login", "login"),
    );
    p
}

fn bruno_files(p: &TestProject) -> Vec<String> {
    tree(&p.root().join("bruno"))
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| n.ends_with(".bru"))
        .collect()
}

// ------------------------------------------------------------ the cycle

#[test]
fn starts_with_a_full_sync_and_reports_readiness() {
    let p = project();
    let mut w = Watching::start(&p);
    assert!(w.seen.iter().any(|l| l == "Initial sync:"), "{:?}", w.seen);
    assert!(
        w.seen.iter().any(|l| l.contains("Created: 4")),
        "{:?}",
        w.seen
    );
    assert!(
        w.seen
            .iter()
            .any(|l| l.starts_with("Watching src (3 directories)")),
        "{:?}",
        w.seen
    );
    assert_eq!(bruno_files(&p).len(), 4);
    w.stop_with("INT");
}

#[test]
fn a_modified_controller_updates_only_its_requests() {
    let p = project();
    let mut w = Watching::start(&p);
    let auth_before = fs::read(p.root().join("bruno/auth/login.bru")).unwrap();

    p.write(
        "src/users/users.controller.ts",
        &controller("users", "changed/:id", "findOne"),
    );
    let counts = w.counts_after("src/users/users.controller.ts");
    assert!(
        counts.contains("Updated: 1")
            && counts.contains("Created: 0")
            && counts.contains("Deleted: 0"),
        "{counts}"
    );
    assert!(bruno(&p, "users/find-one.bru").contains("changed/:id"));
    assert_eq!(
        fs::read(p.root().join("bruno/auth/login.bru")).unwrap(),
        auth_before,
        "other requests are untouched"
    );
    w.stop_with("INT");
}

#[test]
fn a_new_controller_in_a_new_directory_is_picked_up() {
    let p = project();
    let mut w = Watching::start(&p);
    // The directory does not exist yet: its watch must be registered on the fly.
    p.write(
        "src/billing/invoices/invoices.controller.ts",
        &controller("invoices", "list", "findAll"),
    );
    let counts = w.counts_after("src/billing/invoices/invoices.controller.ts");
    assert!(counts.contains("Created: 2"), "{counts}");

    // And a file created later in that same new directory is seen too.
    p.write(
        "src/billing/invoices/more.controller.ts",
        &controller("more", "m", "findMore"),
    );
    let counts = w.counts_after("src/billing/invoices/more.controller.ts");
    assert!(counts.contains("Created: 2"), "{counts}");
    w.stop_with("INT");
}

#[test]
fn a_removed_controller_deletes_its_requests() {
    let p = project();
    let mut w = Watching::start(&p);
    fs::remove_file(p.root().join("src/auth/auth.controller.ts")).unwrap();
    let counts = w.counts_after("src/auth/auth.controller.ts");
    assert!(counts.contains("Deleted: 2"), "{counts}");
    assert!(!p.root().join("bruno/auth").exists());
    assert!(p.root().join("bruno/users/find-one.bru").is_file());
    w.stop_with("INT");
}

#[test]
fn a_removed_directory_deletes_everything_below_it() {
    let p = project();
    p.write("src/mods/a/a.controller.ts", &controller("a", "x", "findA"));
    p.write("src/mods/b/b.controller.ts", &controller("b", "y", "findB"));
    let mut w = Watching::start(&p);
    fs::remove_dir_all(p.root().join("src/mods")).unwrap();
    let counts = w.counts_after("src/mods/a/a.controller.ts");
    assert!(counts.contains("Deleted:"), "{counts}");
    w.sentinel(&p);
    assert!(
        !bruno_files(&p)
            .iter()
            .any(|f| f.starts_with("a/") || f.starts_with("b/")),
        "{:?}",
        bruno_files(&p)
    );
    assert!(p.root().join("bruno/users/find-one.bru").is_file());
    w.stop_with("INT");
}

// ------------------------------------------------------------ editors and bursts

#[test]
fn an_editor_style_atomic_save_is_one_logical_change() {
    let p = project();
    let mut w = Watching::start_with(&p, &["--debounce", "300"]);
    let target = p.root().join("src/users/users.controller.ts");
    let temp = p.root().join("src/users/.users.controller.ts.swp-tmp");
    // write temp, fsync-ish touch, rename over the original, touch again
    for _ in 0..3 {
        fs::write(&temp, controller("users", "saved", "findOne")).unwrap();
        fs::rename(&temp, &target).unwrap();
        fs::write(&target, controller("users", "saved0", "findOne")).unwrap();
    }
    w.counts_after("src/users/users.controller.ts");
    w.sentinel(&p);
    assert_eq!(
        w.changed_lines()
            .iter()
            .filter(|l| **l == "Changed: src/users/users.controller.ts")
            .count(),
        1,
        "one save, one synchronization: {:?}",
        w.seen
    );
    assert!(bruno(&p, "users/find-one.bru").contains("saved0"));
    w.stop_with("INT");
}

#[test]
fn many_files_at_once_all_end_up_synchronized() {
    let p = project();
    let mut w = Watching::start(&p);
    for i in 0..40 {
        p.write(
            format!("src/bulk/c{i}.controller.ts"),
            &controller(&format!("bulk{i}"), "r", &format!("find{i}")),
        );
    }
    w.sentinel(&p);
    // The sentinel was written last; everything before it must be in place
    // once the process has reported the sentinel and gone idle.
    w.sentinel(&p);
    assert_eq!(
        bruno_files(&p).len(),
        4 + 80 + 4,
        "{:?}",
        bruno_files(&p).len()
    );
    w.stop_with("INT");
}

// ------------------------------------------------------------ source = "."

#[test]
fn the_collection_inside_the_source_tree_is_neither_watched_nor_a_source() {
    let p = TestProject::new();
    p.write("nestbruno.toml", "source = \".\"\n");
    p.write(
        "users/users.controller.ts",
        &controller("users", ":id", "findOne"),
    );
    // Directories that must never be watched, and a directory that only exists
    // after the first synchronization (the collection itself).
    p.write("node_modules/dep/lib/x.js", "x");
    p.write(".git/objects/aa/bb", "x");
    p.write("dist/out/x.js", "x");
    // The collection already exists when watching starts, so registering
    // watches could (wrongly) pick up its directories.
    assert!(p.run(&["sync"]).status.success());
    assert!(p.root().join("bruno/users").is_dir());
    let mut w = Watching::start(&p);
    // `.`, `users`: the collection (bruno/, bruno/users), node_modules, .git and
    // dist are not watched.
    assert!(
        w.seen
            .iter()
            .any(|l| l.starts_with("Watching . (2 directories)")),
        "{:?}",
        w.seen
    );

    let edited = bruno(&p, "users/find-one.bru").replace("auth: inherit", "auth: none");
    p.write("bruno/users/find-one.bru", &edited);
    p.write("bruno/extra/new.bru", "meta {\n}\n");
    p.write(
        "node_modules/dep/index.controller.ts",
        &controller("dep", "d", "findDep"),
    );
    p.write(
        "users/users.controller.ts",
        &controller("users", "x/:id", "findOne"),
    );
    w.counts_after("users/users.controller.ts");
    w.sentinel_at(&p, "zz1.controller.ts");
    assert_eq!(
        w.changed_lines(),
        [
            "Changed: users/users.controller.ts",
            "Changed: zz1.controller.ts"
        ],
        "{:?}",
        w.seen
    );
    // The user's edit of the request survives, and the changed route is reported
    // as a conflict instead of being merged or overwritten.
    assert_eq!(bruno(&p, "users/find-one.bru"), edited);
    assert!(
        w.seen
            .iter()
            .any(|l| l.contains("CONFLICT") && l.contains("find-one.bru")),
        "{:?}",
        w.seen
    );
    w.stop_with("INT");
}

// ------------------------------------------------------------ what must NOT trigger

#[test]
fn unrelated_files_and_collection_edits_trigger_nothing() {
    let p = project();
    let mut w = Watching::start(&p);
    p.write("src/users/users.service.ts", "export class S {}");
    p.write("src/notes.txt", "hello");
    p.write("src/users/data.json", "{}");
    p.write(
        "src/dist/build.controller.ts",
        &controller("built", "b", "findBuilt"),
    );
    p.write(
        "src/node_modules/pkg/x.controller.ts",
        &controller("dep", "d", "findDep"),
    );
    p.write("README.md", "outside the source directory");
    // Editing a generated request must not trigger synchronization either (no feedback loop).
    let edited = bruno(&p, "users/find-one.bru").replace("auth: inherit", "auth: none");
    p.write("bruno/users/find-one.bru", &edited);
    p.write("bruno/environments/local.bru", "vars {\n  a: 1\n}\n");

    w.sentinel(&p);
    assert_eq!(
        w.changed_lines(),
        ["Changed: src/zz1.controller.ts"],
        "{:?}",
        w.seen
    );
    assert_eq!(bruno(&p, "users/find-one.bru"), edited);
    w.stop_with("INT");
}

#[test]
fn our_own_writes_do_not_trigger_further_synchronization() {
    let p = project();
    let mut w = Watching::start(&p);
    p.write(
        "src/users/users.controller.ts",
        &controller("users", "x/:id", "findOne"),
    );
    w.counts_after("src/users/users.controller.ts");
    // Had the update (a write into the collection) re-triggered itself, a second
    // report for the same file would precede the sentinel.
    w.sentinel(&p);
    assert_eq!(w.changed_lines().len(), 2, "{:?}", w.seen);
    w.stop_with("INT");
}

// ------------------------------------------------------------ errors keep it alive

#[test]
fn a_broken_file_keeps_the_watcher_alive_and_destroys_nothing() {
    let p = project();
    let mut w = Watching::start(&p);
    let before = bruno_files(&p);

    p.write(
        "src/auth/auth.controller.ts",
        "@Controller('auth') export class AuthController { @Post('login' login() {} }",
    );
    w.wait_changed("src/auth/auth.controller.ts");
    let err = w.wait_stderr("a parse error", |l| l.contains("unable to parse"));
    assert!(err.contains("src/auth/auth.controller.ts:1:"), "{err}");
    w.wait_stderr("the retention notice", |l| {
        l.contains("keeping the previously known routes")
    });
    assert_eq!(
        bruno_files(&p),
        before,
        "no destructive synchronization on incomplete analysis"
    );

    // Another change while the first is still broken: processed, no deletions.
    p.write(
        "src/users/users.controller.ts",
        &controller("users", "other/:id", "findOne"),
    );
    let counts = w.counts_after("src/users/users.controller.ts");
    assert!(
        counts.contains("Updated: 1") && counts.contains("Deleted: 0"),
        "{counts}"
    );

    // Fixed: synchronization of that file resumes.
    p.write(
        "src/auth/auth.controller.ts",
        &controller("auth", "login2", "login"),
    );
    let counts = w.counts_after("src/auth/auth.controller.ts");
    assert!(counts.contains("Updated: 1"), "{counts}");
    assert!(bruno(&p, "auth/login.bru").contains("login2"));
    w.stop_with("INT");
}

#[test]
fn a_request_edited_in_bruno_is_reported_as_a_conflict_and_kept() {
    let p = project();
    let mut w = Watching::start(&p);
    let edited = bruno(&p, "users/find-one.bru").replace("auth: inherit", "auth: bearer");
    p.write("bruno/users/find-one.bru", &edited);

    p.write(
        "src/users/users.controller.ts",
        &controller("users", "changed/:id", "findOne"),
    );
    w.wait_changed("src/users/users.controller.ts");
    let conflict = w.wait_for("a conflict", |l| l.contains("CONFLICT"));
    assert!(
        conflict.contains("users/find-one.bru") && conflict.contains("was modified"),
        "{conflict}"
    );
    w.wait_for("the hint", |l| l.contains("what to do:"));
    assert_eq!(bruno(&p, "users/find-one.bru"), edited);
    w.stop_with("INT");
}

// ------------------------------------------------------------ moves

#[test]
fn moving_a_controller_directory_updates_in_place_without_duplicates() {
    let p = project();
    let mut w = Watching::start(&p);
    let before = bruno_files(&p);
    fs::create_dir_all(p.root().join("src/platform")).unwrap();
    fs::rename(
        p.root().join("src/users"),
        p.root().join("src/platform/users"),
    )
    .unwrap();
    w.wait_changed("src/platform/users/users.controller.ts");
    w.sentinel(&p);
    let after: Vec<_> = bruno_files(&p)
        .into_iter()
        .filter(|f| !f.starts_with("zz"))
        .collect();
    let before_names: std::collections::BTreeSet<_> = before.iter().collect();
    // The users requests were updated in place: no new folder, nothing lost.
    assert!(before_names.iter().all(|f| after.contains(f)), "{after:?}");
    assert!(bruno(&p, "users/find-one.bru").contains("src/platform/users/users.controller.ts"));
    w.stop_with("INT");
}

// ------------------------------------------------------------ configuration

#[test]
fn a_changed_configuration_is_reloaded_and_applied() {
    let p = project();
    p.write("nestbruno.toml", "base_url = \"http://one\"\n");
    let mut w = Watching::start(&p);
    assert!(bruno(&p, "users/find-one.bru").contains("url: http://one/"));
    p.write("nestbruno.toml", "base_url = \"http://two\"\n");
    w.wait_for("the reload notice", |l| {
        l == "Configuration changed; reloaded."
    });
    w.wait_for("the initial sync of the new session", |l| {
        l == "Initial sync:"
    });
    w.wait_for("readiness", |l| l.starts_with("Watching "));
    assert!(bruno(&p, "users/find-one.bru").contains("url: http://two/"));
    w.stop_with("INT");
}

#[test]
fn an_invalid_configuration_is_reported_and_the_old_one_stays_active() {
    let p = project();
    let mut w = Watching::start(&p);
    p.write("nestbruno.toml", "bruno = \"../outside\"\n");
    let err = w.wait_stderr("a configuration error", |l| l.contains("nestbruno.toml"));
    assert!(err.contains("keeping the previous configuration"), "{err}");
    p.write(
        "src/users/users.controller.ts",
        &controller("users", "still/:id", "findOne"),
    );
    w.counts_after("src/users/users.controller.ts");
    assert!(bruno(&p, "users/find-one.bru").contains("still/:id"));
    w.stop_with("INT");
}

// ------------------------------------------------------------ shutdown

#[test]
fn ctrl_c_stops_cleanly_with_exit_code_zero() {
    let p = project();
    let mut w = Watching::start(&p);
    let status = w.stop_with("INT");
    assert_eq!(status.code(), Some(0));
    assert_eq!(w.seen.last().map(String::as_str), Some("Stopped."));
}

#[test]
fn sigterm_and_sighup_also_stop_cleanly() {
    for signal in ["TERM", "HUP"] {
        let p = project();
        let mut w = Watching::start(&p);
        let status = w.stop_with(signal);
        assert_eq!(status.code(), Some(0), "{signal}");
        assert_eq!(
            w.seen.last().map(String::as_str),
            Some("Stopped."),
            "{signal}"
        );
    }
}

#[test]
fn shutdown_leaves_no_partial_or_temporary_files() {
    let p = project();
    let mut w = Watching::start(&p);
    for i in 0..10 {
        p.write(
            format!("src/burst/c{i}.controller.ts"),
            &controller(&format!("burst{i}"), "r", "find"),
        );
    }
    w.stop_with("INT");
    for (name, content) in tree(&p.root().join("bruno")) {
        assert!(!name.contains(".nestbruno-"), "leftover temp file {name}");
        if name.ends_with(".bru") {
            let text = String::from_utf8(content).unwrap();
            assert!(
                text.contains("NestBruno: v1") && text.ends_with("}\n"),
                "{name} is not complete"
            );
        }
    }
}

// ------------------------------------------------------------ startup problems

#[test]
fn a_missing_source_directory_is_a_clear_error_not_a_hang() {
    let p = TestProject::new();
    let output = p.run(&["watch"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(common::stderr(&output).contains("does not exist"));
}

#[test]
fn nothing_is_written_to_the_sources() {
    let p = project();
    let before = tree(&p.root().join("src"));
    let mut w = Watching::start(&p);
    p.write(
        "src/users/users.controller.ts",
        &controller("users", "z/:id", "findOne"),
    );
    w.counts_after("src/users/users.controller.ts");
    let changed: Vec<_> = tree(&p.root().join("src"))
        .into_iter()
        .filter(|(n, c)| !before.contains(&(n.clone(), c.clone())))
        .map(|(n, _)| n)
        .collect();
    assert_eq!(
        changed,
        ["users/users.controller.ts"],
        "only the file the test itself wrote differs"
    );
    let _ = (Path::new(""), w.stderr_text());
    w.stop_with("INT");
}
