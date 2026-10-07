//! NestJS source analysis: reads discovered files and turns them into the
//! route IR ([`crate::route`]).
//!
//! ```text
//! ControllerFile list ─▶ bounded read ─▶ extract_controllers ─▶ Controller IR
//! ```
//!
//! Each file is read once and parsed once; the AST is dropped as soon as the
//! file's IR has been built. File problems never abort the run: they become
//! diagnostics, and any `Severity::Error` diagnostic means the result for that
//! file is *unknown* (not "empty"), which later phases must respect.

pub mod extract;

use std::fs::{self, File};
use std::io::Read;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::thread;

use crate::diag::{Diagnostic, Severity};
use crate::discovery::ControllerFile;
use crate::error::Error;
use crate::project::Project;
use crate::route::Controller;
use extract::{MAX_SOURCE_BYTES, PARSER_STACK_BYTES, extract_controllers, too_large};

/// The analysis of a set of files.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Controllers in discovery (sorted file) order, then source order.
    pub controllers: Vec<Controller>,
    /// Problems in the same order. Any [`Severity::Error`] means the routes of
    /// the named file are unknown.
    pub diagnostics: Vec<Diagnostic>,
}

impl Analysis {
    /// Whether any file could not be analyzed.
    pub fn is_incomplete(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// Analyze `files` (paths relative to the project root).
///
/// Analysis runs on one worker thread with a stack large enough for the
/// parser's worst case (see [`PARSER_STACK_BYTES`]). The stack is virtual
/// memory that is only committed if a file is actually deeply nested.
pub fn analyze_files(project: &Project, files: &[ControllerFile]) -> Result<Analysis, Error> {
    thread::scope(|scope| {
        let worker = thread::Builder::new()
            .name("nestbruno-analyze".to_owned())
            .stack_size(PARSER_STACK_BYTES)
            .spawn_scoped(scope, || analyze_all(project.root(), files))
            .map_err(Error::AnalysisThread)?;
        // Panics inside the worker are caught per file; a join error would
        // mean a bug outside that guard.
        worker.join().map_err(|_| {
            Error::AnalysisThread(std::io::Error::other("the analysis worker panicked"))
        })
    })
}

fn analyze_all(root: &Path, files: &[ControllerFile]) -> Analysis {
    let mut analysis = Analysis::default();
    for file in files {
        let source = match read_source(root, &file.path) {
            Ok(source) => source,
            Err(diagnostic) => {
                analysis.diagnostics.push(diagnostic);
                continue;
            }
        };

        // Defence in depth: the parser is third-party code fed untrusted
        // input. A panic in it must cost one file, not the whole run.
        match guarded(|| extract_controllers(&file.path, &source)) {
            Ok(mut result) => {
                if result.controllers.is_empty() && result.diagnostics.is_empty() {
                    analysis.diagnostics.push(Diagnostic::new(
                        Severity::Note,
                        Some(file.path.clone()),
                        "no @Controller class found",
                    ));
                }
                analysis.controllers.append(&mut result.controllers);
                analysis.diagnostics.append(&mut result.diagnostics);
            }
            Err(message) => analysis.diagnostics.push(Diagnostic::new(
                Severity::Error,
                Some(file.path.clone()),
                format!("internal error while analyzing this file ({message}); please report a bug. No routes were extracted from it"),
            )),
        }
    }
    analysis
}

/// Read one source file: regular files only, bounded size, UTF-8.
fn read_source(root: &Path, relative: &Path) -> Result<String, Diagnostic> {
    let error =
        |message: String| Diagnostic::new(Severity::Error, Some(relative.to_owned()), message);
    let path = root.join(relative);

    // Discovery only reports regular files, but the tree may have changed
    // since; never read through a link or a special file.
    let metadata =
        fs::symlink_metadata(&path).map_err(|err| error(format!("cannot read file: {err}")))?;
    if !metadata.is_file() {
        return Err(error("not a regular file; skipped".to_owned()));
    }
    if metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err(too_large(
            relative,
            usize::try_from(metadata.len()).unwrap_or(usize::MAX),
        ));
    }

    // The file may grow after the metadata check: read at most one byte more
    // than the limit.
    let mut text = String::new();
    File::open(&path)
        .and_then(|file| {
            file.take(MAX_SOURCE_BYTES as u64 + 1)
                .read_to_string(&mut text)
        })
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::InvalidData => error("not valid UTF-8; skipped".to_owned()),
            _ => error(format!("cannot read file: {err}")),
        })?;
    if text.len() > MAX_SOURCE_BYTES {
        return Err(too_large(relative, text.len()));
    }
    Ok(text)
}

/// Run `work`, converting a panic into its message.
fn guarded<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(work)).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_returns_the_value_when_there_is_no_panic() {
        assert_eq!(guarded(|| 7), Ok(7));
    }

    #[test]
    fn guarded_converts_panics_into_messages() {
        // Silence the default hook so the expected panics do not clutter output.
        let hook = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        let literal = guarded(|| -> () { panic!("boom") });
        let formatted = guarded(|| -> () { panic!("{}", String::from("formatted")) });
        panic::set_hook(hook);
        assert_eq!(literal, Err("boom".to_owned()));
        assert_eq!(formatted, Err("formatted".to_owned()));
    }
}
