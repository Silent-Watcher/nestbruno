//! Diagnostics: recoverable problems and user-facing progress output.
//!
//! Library code never prints. It returns [`Diagnostic`]s; the CLI decides how
//! to present them through a [`Reporter`].

use std::fmt;
use std::io::Write;
use std::path::PathBuf;

/// How serious a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational; only shown with `--verbose`.
    Note,
    /// The result is complete, but something is unsupported or suspicious.
    Warning,
    /// The result is **incomplete** for the affected path (for example an
    /// unreadable directory or a file that failed to parse). Later phases must
    /// treat the state of that path as unknown rather than as "empty".
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        })
    }
}

/// A recoverable problem, optionally attached to a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Path relative to the project root when known.
    pub path: Option<PathBuf>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(severity: Severity, path: Option<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            severity,
            path,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.severity)?;
        if let Some(path) = &self.path {
            write!(f, "{}: ", path.display())?;
        }
        f.write_str(&self.message)
    }
}

/// Writes diagnostics and progress messages to stderr.
#[derive(Debug, Clone, Copy)]
pub struct Reporter {
    verbose: bool,
}

impl Reporter {
    pub fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    pub fn is_verbose(&self) -> bool {
        self.verbose
    }

    /// Print a diagnostic. Notes are suppressed unless verbose.
    pub fn report(&self, diagnostic: &Diagnostic) {
        if diagnostic.severity == Severity::Note && !self.verbose {
            return;
        }
        self.write_line(format_args!("{diagnostic}"));
    }

    /// Print a free-form progress line, only in verbose mode.
    pub fn verbose(&self, message: fmt::Arguments<'_>) {
        if self.verbose {
            self.write_line(message);
        }
    }

    fn write_line(&self, message: fmt::Arguments<'_>) {
        // If stderr is closed there is nowhere left to report the failure, and
        // panicking (as `eprintln!` would) is worse than staying quiet.
        let _ = writeln!(std::io::stderr().lock(), "{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_severity_path_and_message() {
        let d = Diagnostic::new(
            Severity::Warning,
            Some(PathBuf::from("src/a.ts")),
            "something odd",
        );
        assert_eq!(d.to_string(), "warning: src/a.ts: something odd");
    }

    #[test]
    fn display_without_path() {
        let d = Diagnostic::new(Severity::Error, None, "boom");
        assert_eq!(d.to_string(), "error: boom");
    }

    #[test]
    fn severities_order_by_seriousness() {
        assert!(Severity::Note < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
    }
}
