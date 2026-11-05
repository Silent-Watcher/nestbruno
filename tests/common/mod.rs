//! Shared helpers for integration tests.
//!
//! Scoped allowances (this file only):
//! - `dead_code`: every integration-test binary uses a different subset.
//! - `clippy::expect_used`: a panic with a message is the correct failure mode
//!   for broken test setup, and clippy's test exemption does not cover helpers.
#![allow(dead_code, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A throwaway project directory that is deleted when dropped.
pub struct TestProject {
    dir: TempDir,
}

impl TestProject {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("create temp dir"),
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Write `contents` to `rel`, creating parent directories.
    pub fn write(&self, rel: impl AsRef<Path>, contents: &str) -> PathBuf {
        let path = self.dir.path().join(rel);
        fs::create_dir_all(path.parent().expect("file has a parent")).expect("create dirs");
        fs::write(&path, contents).expect("write file");
        path
    }

    /// Overwrite an existing file (or create it).
    pub fn write_over(&self, rel: impl AsRef<Path>, contents: &str) {
        fs::write(self.dir.path().join(rel), contents).expect("overwrite file");
    }

    /// Copy a directory tree (a fixture) into the project.
    pub fn copy_tree(&self, from: &Path, to_rel: impl AsRef<Path>) {
        fn copy(from: &Path, to: &Path) {
            fs::create_dir_all(to).expect("create dir");
            for entry in fs::read_dir(from).expect("read fixture dir") {
                let entry = entry.expect("fixture entry");
                let target = to.join(entry.file_name());
                if entry.file_type().expect("file type").is_dir() {
                    copy(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).expect("copy fixture file");
                }
            }
        }
        copy(from, &self.dir.path().join(to_rel));
    }

    /// Run the real `nestbruno` binary with `-C <root>` and `args`.
    pub fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_nestbruno"))
            .arg("-C")
            .arg(self.root())
            .args(args)
            .output()
            .expect("run nestbruno")
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Run discovery through the library API, panicking on fatal errors.
pub fn discover(project: &TestProject) -> nestbruno::discovery::Discovery {
    let project = nestbruno::project::Project::open(project.root()).expect("open project");
    nestbruno::discovery::discover_controllers(&project).expect("discover controllers")
}

/// Every regular file below `dir` as `(relative path with '/', content)`,
/// sorted by path. Directories themselves are not listed.
pub fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).expect("under base");
                out.push((
                    rel.to_string_lossy().replace('\\', "/"),
                    fs::read(&path).expect("read file"),
                ));
            }
        }
    }
    let mut out = Vec::new();
    if dir.exists() {
        walk(dir, dir, &mut out);
    }
    out.sort();
    out
}

/// Absolute path of a fixture under `tests/fixtures`.
pub fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// Read a UTF-8 file, panicking with its path on failure.
pub fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

// --- git fixtures -----------------------------------------------------------
//
// Every repository is created in a throwaway temp directory through libgit2;
// no test touches the developer's own git configuration or repositories, and
// nothing is ever committed outside a temp dir. Staging goes through the git2
// API (blobs written directly into the index), so a test can stage content
// that differs from the working tree.

use git2::{IndexEntry, IndexTime, Repository, Signature};

/// Initialize a repository at the project root.
pub fn init_git(project: &TestProject) -> Repository {
    Repository::init(project.root()).expect("init repo")
}

/// Stage `rel` (worktree-relative) with `content` without touching the
/// working tree.
pub fn stage_buffer(repo: &Repository, rel: &str, content: &[u8]) {
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
pub fn stage_file(repo: &Repository, rel: &str) {
    let mut index = repo.index().expect("read index");
    index.add_path(Path::new(rel)).expect("add path");
    index.write().expect("write index");
}

/// Remove `rel` from the index (a staged deletion).
pub fn unstage(repo: &Repository, rel: &str) {
    let mut index = repo.index().expect("read index");
    index.remove_path(Path::new(rel)).expect("remove path");
    index.write().expect("write index");
}

/// Commit the whole index so HEAD exists.
pub fn commit_all(repo: &Repository, message: &str) {
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
