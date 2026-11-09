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
use std::path::{Path, PathBuf};
use std::thread;

use crate::config::Config;
use crate::diag::{Diagnostic, Severity};
use crate::discovery::{ControllerFile, Matcher};
use crate::error::Error;
use crate::project::Project;
use crate::resolve::ResolverContext;
use crate::route::Controller;
use extract::{FileAnalysis, MAX_SOURCE_BYTES, PARSER_STACK_BYTES, extract_controllers, too_large};

/// The per-run project facts threaded through analysis: where the project
/// lives, its configuration, and the run-scoped cross-file resolver state.
///
/// `None`-valued project context (staged analysis, most unit tests) means no
/// cross-file DTO resolution is performed.
pub struct ProjectContext<'a> {
    /// Project root directory.
    pub root: &'a Path,
    /// Source directory prefix (e.g., "src").
    pub source_prefix: &'a Path,
    /// Configuration.
    pub config: &'a Config,
    /// Discovery matcher for exclusion checks.
    pub matcher: &'a Matcher,
    /// Shared run state for cross-file DTO resolution.
    pub resolver: &'a mut ResolverContext,
}

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
    let mut analysis = Analysis::default();
    for (_, mut result) in analyze_each(project, files)? {
        analysis.controllers.append(&mut result.controllers);
        analysis.diagnostics.append(&mut result.diagnostics);
    }
    Ok(analysis)
}

/// Analyze in-memory sources instead of files.
///
/// The source of the content is irrelevant to the analysis: staged git
/// synchronization provides the staged blob of every relevant file this way,
/// and future inputs would plug in the same place. Paths are project-relative
/// and become the [`crate::route::SourceLocation`] file of every extracted
/// route, exactly as with [`analyze_files`]. The content must already be
/// UTF-8 and within [`MAX_SOURCE_BYTES`]; the staged loader enforces this.
pub fn analyze_sources(sources: &[(PathBuf, String)]) -> Result<Analysis, Error> {
    let mut analysis = Analysis::default();
    for (_, mut result) in analyze_contents(sources)? {
        analysis.controllers.append(&mut result.controllers);
        analysis.diagnostics.append(&mut result.diagnostics);
    }
    Ok(analysis)
}

/// Like [`analyze_files`], but keeps the result of each file separate (in the
/// order of `files`). Incremental synchronization needs to know which file
/// produced which controllers.
pub fn analyze_each(
    project: &Project,
    files: &[ControllerFile],
) -> Result<Vec<(PathBuf, FileAnalysis)>, Error> {
    let sources: Vec<(PathBuf, Option<String>)> =
        files.iter().map(|file| (file.path.clone(), None)).collect();
    let root = project.root();
    let source_prefix = project.config().source.clone();
    let config = project.config();
    let matcher = Matcher::new(config).ok();
    // One resolver context per analysis run: every file below shares the
    // module cache, budgets and diagnostics, so a dependency imported by
    // several controllers is read and parsed once.
    let mut resolver_context = ResolverContext::new(root);
    run_analysis(sources, move |relative, content| {
        let mut project_context = matcher.as_ref().map(|matcher| ProjectContext {
            root,
            source_prefix: &source_prefix,
            config,
            matcher,
            resolver: &mut resolver_context,
        });
        match content {
            Some(source) => analyze_content(relative, &source, project_context.as_mut()),
            None => {
                let source = read_source(root, relative);
                match source {
                    Ok(source) => analyze_content(relative, &source, project_context.as_mut()),
                    Err(diagnostic) => FileAnalysis {
                        controllers: Vec::new(),
                        diagnostics: vec![diagnostic],
                    },
                }
            }
        }
    })
}

/// Like [`analyze_sources`], but keeps the result of each source separate.
fn analyze_contents(sources: &[(PathBuf, String)]) -> Result<Vec<(PathBuf, FileAnalysis)>, Error> {
    let owned: Vec<(PathBuf, Option<String>)> = sources
        .iter()
        .map(|(path, content)| (path.clone(), Some(content.clone())))
        .collect();
    run_analysis(owned, |relative, content| {
        // The loader guarantees `Some` for in-memory sources.
        match content {
            Some(source) => analyze_content(relative, &source, None),
            None => FileAnalysis {
                controllers: Vec::new(),
                diagnostics: vec![Diagnostic::new(
                    Severity::Error,
                    Some(relative.to_owned()),
                    "internal error: staged content missing; please report a bug".to_owned(),
                )],
            },
        }
    })
}

/// Run `analyze` for every (path, optional content) on the dedicated worker
/// thread with the parser-sized stack. One worker, so the closure may hold
/// run-scoped mutable state ([`FnMut`], [`Send`], no `Sync` needed).
fn run_analysis(
    inputs: Vec<(PathBuf, Option<String>)>,
    mut analyze: impl FnMut(&Path, Option<String>) -> FileAnalysis + Send,
) -> Result<Vec<(PathBuf, FileAnalysis)>, Error> {
    thread::scope(|scope| {
        let worker = thread::Builder::new()
            .name("nestbruno-analyze".to_owned())
            .stack_size(PARSER_STACK_BYTES)
            .spawn_scoped(scope, || {
                inputs
                    .iter()
                    .map(|(path, content)| {
                        let result = analyze(path, content.clone());
                        (path.clone(), result)
                    })
                    .collect()
            })
            .map_err(Error::AnalysisThread)?;
        // Panics inside the worker are caught per file; a join error would
        // mean a bug outside that guard.
        worker.join().map_err(|_| {
            Error::AnalysisThread(std::io::Error::other("the analysis worker panicked"))
        })
    })
}

/// Analyze content that was already read (from disk or elsewhere).
fn analyze_content(
    relative: &Path,
    source: &str,
    project: Option<&mut ProjectContext<'_>>,
) -> FileAnalysis {
    // Defence in depth: the parser is third-party code fed untrusted input. A
    // panic in it must cost one file, not the whole run.
    match guarded(|| extract_controllers(relative, source, project)) {
        Ok(mut result) => {
            if result.controllers.is_empty() && result.diagnostics.is_empty() {
                result.diagnostics.push(Diagnostic::new(
                    Severity::Note,
                    Some(relative.to_owned()),
                    "no @Controller class found",
                ));
            }
            result
        }
        Err(message) => FileAnalysis {
            controllers: Vec::new(),
            diagnostics: vec![Diagnostic::new(
                Severity::Error,
                Some(relative.to_owned()),
                format!(
                    "internal error while analyzing this file ({message}); please report a bug. No routes were extracted from it"
                ),
            )],
        },
    }
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
