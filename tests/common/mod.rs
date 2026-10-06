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
