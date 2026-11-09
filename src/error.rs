//! The single error type for NestBruno and its mapping to process exit codes.
//!
//! Fatal problems (bad configuration, missing project, I/O failures that make a
//! command meaningless) are `Error`s. Problems confined to a single file are
//! *not* errors; they are reported as [`crate::diag::Diagnostic`]s so that the
//! rest of the project can still be processed.

use std::path::PathBuf;

/// Exit codes used by the CLI. `2` is reserved for usage errors, which clap
/// reports itself (and which manual argument validation mirrors).
pub mod exit_code {
    /// Unspecified runtime failure (I/O, output serialization). `verify`
    /// also uses this code when the collection is out of sync (CI failure
    /// signal, distinct from the "incomplete" code 4), and `explain` when no
    /// route matches the selector.
    pub const FAILURE: u8 = 1;
    /// Usage errors (clap, or a rejected command-argument combination).
    pub const USAGE: u8 = 2;
    /// The configuration or the project layout is invalid.
    pub const CONFIG: u8 = 3;
    /// The command finished, but some paths could not be processed (see the
    /// `error:` diagnostics on stderr), so the result is incomplete.
    pub const INCOMPLETE: u8 = 4;
}

/// Fatal errors. Every message states what failed, where, and what to do.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot access {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot write command output: {0}")]
    Output(#[source] std::io::Error),

    #[error("cannot serialize JSON output: {0}")]
    Json(#[source] serde_json::Error),

    #[error("invalid configuration in {}: {source}", path.display())]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid configuration in {}: {message}", path.display())]
    ConfigInvalid { path: PathBuf, message: String },

    #[error(
        "no NestBruno project found at or above {}: add a nestbruno.toml, run `nestbruno init`, or pass --project <DIR>",
        start.display()
    )]
    ProjectNotFound { start: PathBuf },

    #[error(
        "source directory {} does not exist or is not a directory: create it or set `source` in nestbruno.toml",
        path.display()
    )]
    SourceDirMissing { path: PathBuf },

    #[error("invalid {kind} pattern {pattern:?} in the nestbruno.toml `{kind}` setting: {source}")]
    InvalidPattern {
        kind: &'static str,
        pattern: String,
        #[source]
        source: globset::Error,
    },

    #[error(
        "source directory {} resolves to {}, which is outside the project root {}: symlinks out of the project are not followed; set `source` to a directory inside the project",
        configured.display(), resolved.display(), root.display()
    )]
    SourceOutsideProject {
        configured: PathBuf,
        resolved: PathBuf,
        root: PathBuf,
    },

    #[error("refusing to write {}: {reason}", path.display())]
    UnsafeOutput { path: PathBuf, reason: String },

    #[error("cannot start the source analysis worker thread: {0}")]
    AnalysisThread(#[source] std::io::Error),

    #[error("cannot watch for changes: {0}")]
    Watch(String),

    #[error("{} already exists: refusing to overwrite it", path.display())]
    AlreadyExists { path: PathBuf },

    #[error(
        "no git repository found at or above {}: staged synchronization analyzes the git index; run inside a repository or pass --project <DIR>",
        start.display()
    )]
    GitNotARepository { start: PathBuf },

    #[error("no route matches {selector}")]
    RouteNotFound { selector: String },

    #[error(
        "{count} routes match {selector}; narrow the selection with --file or use --controller/--handler. Matching routes: {list}"
    )]
    AmbiguousRoutes {
        selector: String,
        count: usize,
        list: String,
    },

    #[error("{message}")]
    Usage { message: String },

    #[error(
        "cannot read the git repository at {}: {message}",
        repo.display()
    )]
    GitRepository { repo: PathBuf, message: String },
}

impl Error {
    /// The process exit code for this error.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::ConfigParse { .. }
            | Self::ConfigInvalid { .. }
            | Self::ProjectNotFound { .. }
            | Self::SourceDirMissing { .. }
            | Self::InvalidPattern { .. }
            | Self::SourceOutsideProject { .. } => exit_code::CONFIG,
            Self::Io { .. }
            | Self::Output(_)
            | Self::Json(_)
            | Self::AnalysisThread(_)
            | Self::UnsafeOutput { .. }
            | Self::Watch(_)
            | Self::AlreadyExists { .. }
            | Self::GitNotARepository { .. }
            | Self::GitRepository { .. }
            | Self::RouteNotFound { .. }
            | Self::AmbiguousRoutes { .. } => exit_code::FAILURE,
            Self::Usage { .. } => exit_code::USAGE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_errors_use_the_config_exit_code() {
        let err = Error::ConfigInvalid {
            path: PathBuf::from("nestbruno.toml"),
            message: "bad".into(),
        };
        assert_eq!(err.exit_code(), exit_code::CONFIG);
        let err = Error::SourceDirMissing {
            path: PathBuf::from("src"),
        };
        assert_eq!(err.exit_code(), exit_code::CONFIG);
        let err = Error::SourceOutsideProject {
            configured: PathBuf::from("src"),
            resolved: PathBuf::from("/elsewhere"),
            root: PathBuf::from("/project"),
        };
        assert_eq!(err.exit_code(), exit_code::CONFIG);
    }

    #[test]
    fn runtime_errors_use_the_failure_exit_code() {
        let err = Error::Watch("limit reached".into());
        assert_eq!(err.exit_code(), exit_code::FAILURE);
    }

    #[test]
    fn messages_name_the_offending_path() {
        let err = Error::AlreadyExists {
            path: PathBuf::from("a/nestbruno.toml"),
        };
        assert!(err.to_string().contains("a/nestbruno.toml"));
    }
}
