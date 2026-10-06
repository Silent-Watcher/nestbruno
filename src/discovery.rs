//! Controller candidate discovery.
//!
//! # Decision: candidates, not verification
//!
//! Discovery answers "which files *may* contain controllers?" purely from the
//! configuration and file *paths*. It never reads file contents. Whether a
//! candidate really contains an `@Controller` class is decided by the AST
//! analysis in the next phase, which is the only component that parses
//! TypeScript. This keeps discovery cheap (one `read_dir` per directory, no
//! file reads) and avoids two sources of truth about what a controller is.
//! Controllers in unconventionally named files are found by widening `include`.
//!
//! # Pipeline
//!
//! ```text
//! Project ─▶ source root ─▶ directory walk ─▶ prune (hard-skip + exclude)
//!         ─▶ `.ts` filter ─▶ include/exclude match ─▶ sort ─▶ ControllerFile list
//! ```
//!
//! # Pattern semantics
//!
//! - Patterns are globs matched against the path **relative to the source
//!   directory**, so a directory named `build` *above* the source root never
//!   affects matching, and `source = "build"` works.
//! - `*` does not cross `/`; `**` does. Matching is case-sensitive on all
//!   platforms.
//! - **Precedence:** a file is returned only if it matches at least one
//!   `include` pattern **and** no `exclude` pattern. Exclusion always wins.
//! - A directory that matches an exclude pattern (or for which an exclude
//!   pattern matches everything beneath it, like `**/dist/**`) is not entered.
//! - `.git` and `node_modules` directories are never entered, regardless of
//!   configuration: they never hold project sources and may be enormous.
//! - Only files with the extension `.ts` are candidates; `.d.ts` declaration
//!   files are not. (`.tsx`/JavaScript are not NestJS controller sources.)
//!
//! # Symlinks
//!
//! Symlinks (to files or directories) found *inside* the source tree are never
//! followed and are reported as notes. The configured source directory itself
//! may be a symlink only if it resolves to somewhere inside the project root.
//! On Windows, directory junctions are treated like symlinks where the
//! standard library reports them as such; this is untested.
//!
//! # Error policy
//!
//! *Fatal* (the result would be meaningless): invalid include/exclude pattern,
//! missing or non-directory source, source resolving outside the project,
//! unreadable source root. *Recoverable* (reported as `error:` diagnostics,
//! discovery continues, the command exits with the "incomplete" code):
//! an unreadable subdirectory or directory entry, and directories nested deeper
//! than [`MAX_DEPTH`].

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use globset::{Candidate, GlobBuilder, GlobSet, GlobSetBuilder};

use crate::config::Config;
use crate::diag::{Diagnostic, Severity};
use crate::error::Error;
use crate::project::Project;

/// Directories nested deeper than this below the source root are not entered.
/// Guards against pathological trees; real projects are far shallower.
pub const MAX_DEPTH: usize = 64;

/// Directory names that are never entered.
const ALWAYS_SKIPPED_DIRS: [&str; 2] = [".git", "node_modules"];

/// A path component that cannot occur in a real file name (NUL). Used to ask
/// "would an exclude pattern match something inside this directory?".
const PROBE: &str = "\0";

/// A file that may contain NestJS controllers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ControllerFile {
    /// Path relative to the project root.
    pub path: PathBuf,
}

/// The result of a discovery run.
#[derive(Debug, Default)]
pub struct Discovery {
    /// Candidate files, sorted by path (component-wise, byte order on Unix),
    /// without duplicates.
    pub files: Vec<ControllerFile>,
    /// Recoverable problems. Any [`Severity::Error`] means `files` may be
    /// incomplete.
    pub diagnostics: Vec<Diagnostic>,
    /// Number of directories listed; useful to verify that pruning works.
    pub directories_read: usize,
}

impl Discovery {
    /// Whether any path could not be processed (the file list may be incomplete).
    pub fn is_incomplete(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

/// Discover controller candidate files for `project`.
pub fn discover_controllers(project: &Project) -> Result<Discovery, Error> {
    // Compile patterns before touching the filesystem: configuration errors
    // should be reported even when the source tree is also broken.
    let matcher = Matcher::new(project.config())?;
    project.ensure_source_dir()?;
    let source_root = contained_source_root(project)?;

    // Paths are reported relative to the project root, without `.` components.
    let source_prefix: PathBuf = project
        .config()
        .source
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();

    let mut discovery = Walker {
        matcher: &matcher,
        source_prefix: &source_prefix,
        discovery: Discovery::default(),
    }
    .walk(&source_root)?;

    discovery.files.sort_unstable();
    Ok(discovery)
}

/// Resolve the source directory and verify it lies inside the project root.
fn contained_source_root(project: &Project) -> Result<PathBuf, Error> {
    let configured = project.source_root();
    let resolved = configured.canonicalize().map_err(|source| Error::Io {
        path: configured.clone(),
        source,
    })?;
    if resolved.starts_with(project.root()) {
        Ok(resolved)
    } else {
        Err(Error::SourceOutsideProject {
            configured: project.config().source.clone(),
            resolved,
            root: project.root().to_owned(),
        })
    }
}

/// Compiled include/exclude patterns. Built once per discovery run.
struct Matcher {
    include: GlobSet,
    exclude: GlobSet,
}

impl Matcher {
    fn new(config: &Config) -> Result<Self, Error> {
        Ok(Self {
            include: compile("include", &config.include)?,
            exclude: compile("exclude", &config.exclude)?,
        })
    }

    /// Whether `relative` (relative to the source root) is a candidate file.
    fn is_candidate(&self, relative: &Path) -> bool {
        // One normalization of the path serves both sets.
        let candidate = Candidate::new(relative);
        self.include.is_match_candidate(&candidate) && !self.exclude.is_match_candidate(&candidate)
    }

    /// Whether the directory `relative` can be skipped entirely.
    ///
    /// True if an exclude pattern matches the directory itself, or matches a
    /// hypothetical child (so `**/dist/**` prunes `dist` although it does not
    /// match the bare directory path).
    fn is_excluded_dir(&self, relative: &Path) -> bool {
        self.exclude.is_match(relative) || self.exclude.is_match(relative.join(PROBE))
    }
}

fn compile(kind: &'static str, patterns: &[String]) -> Result<GlobSet, Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|source| Error::InvalidPattern {
                kind,
                pattern: pattern.clone(),
                source,
            })?;
        builder.add(glob);
    }
    builder.build().map_err(|source| Error::InvalidPattern {
        kind,
        pattern: patterns.join(", "),
        source,
    })
}

/// `.ts` but not `.d.ts`.
fn is_typescript_source(name: &OsStr) -> bool {
    let path = Path::new(name);
    if path.extension() != Some(OsStr::new("ts")) {
        return false;
    }
    let is_declaration = path
        .file_stem()
        .is_some_and(|stem| Path::new(stem).extension() == Some(OsStr::new("d")));
    !is_declaration
}

struct Walker<'a> {
    matcher: &'a Matcher,
    source_prefix: &'a Path,
    discovery: Discovery,
}

impl Walker<'_> {
    /// Iterative depth-first traversal (no recursion, so depth cannot overflow
    /// the stack).
    fn walk(mut self, source_root: &Path) -> Result<Discovery, Error> {
        // (absolute directory, directory relative to the source root, depth)
        let mut pending = vec![(source_root.to_owned(), PathBuf::new(), 0_usize)];

        while let Some((dir, relative_dir, depth)) = pending.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                // An unreadable source root makes the whole result meaningless.
                Err(source) if depth == 0 => return Err(Error::Io { path: dir, source }),
                Err(err) => {
                    self.problem(
                        Severity::Error,
                        &relative_dir,
                        format!("cannot read directory: {err}"),
                    );
                    continue;
                }
            };
            self.discovery.directories_read += 1;

            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        self.problem(
                            Severity::Error,
                            &relative_dir,
                            format!("cannot read directory entry: {err}"),
                        );
                        continue;
                    }
                };
                self.visit_entry(&entry, &relative_dir, depth, &mut pending);
            }
        }
        Ok(self.discovery)
    }

    fn visit_entry(
        &mut self,
        entry: &fs::DirEntry,
        relative_dir: &Path,
        depth: usize,
        pending: &mut Vec<(PathBuf, PathBuf, usize)>,
    ) {
        let name = entry.file_name();
        // `DirEntry::file_type` does not follow symlinks.
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                self.problem(
                    Severity::Error,
                    &relative_dir.join(&name),
                    format!("cannot determine file type: {err}"),
                );
                return;
            }
        };

        if file_type.is_symlink() {
            self.problem(
                Severity::Note,
                &relative_dir.join(&name),
                "symlink not followed".to_owned(),
            );
        } else if file_type.is_dir() {
            if ALWAYS_SKIPPED_DIRS.iter().any(|skipped| name == *skipped) {
                return;
            }
            let relative = relative_dir.join(&name);
            if self.matcher.is_excluded_dir(&relative) {
                return;
            }
            if depth + 1 > MAX_DEPTH {
                self.problem(
                    Severity::Error,
                    &relative,
                    format!("directory nesting exceeds {MAX_DEPTH} levels; not entered"),
                );
                return;
            }
            pending.push((entry.path(), relative, depth + 1));
        } else if file_type.is_file() && is_typescript_source(&name) {
            // The extension check above is the cheap filter; the relative path
            // (an allocation) is only built for `.ts` files.
            let relative = relative_dir.join(&name);
            if self.matcher.is_candidate(&relative) {
                self.discovery.files.push(ControllerFile {
                    path: self.source_prefix.join(relative),
                });
            }
        }
    }

    /// Record a diagnostic for a path given relative to the source root.
    fn problem(&mut self, severity: Severity, relative: &Path, message: String) {
        self.discovery.diagnostics.push(Diagnostic::new(
            severity,
            Some(self.source_prefix.join(relative)),
            message,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(include: &[&str], exclude: &[&str]) -> Config {
        Config {
            include: include.iter().map(|s| (*s).to_owned()).collect(),
            exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
            ..Config::default()
        }
    }

    fn matcher(include: &[&str], exclude: &[&str]) -> Matcher {
        Matcher::new(&config(include, exclude)).unwrap()
    }

    #[test]
    fn default_config_matches_controller_files_at_any_depth() {
        let m = Matcher::new(&Config::default()).unwrap();
        for path in [
            "users.controller.ts",
            "users/users.controller.ts",
            "a/b/c/d/users.controller.ts",
            "my feature/users.controller.ts",
            "کاربران/users.controller.ts",
        ] {
            assert!(m.is_candidate(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn default_config_rejects_other_files() {
        let m = Matcher::new(&Config::default()).unwrap();
        for path in [
            "users.service.ts",
            "users.controller.js",
            "users.controller.ts.bak",
            "users.controller.spec.ts",
            "controller.ts",
        ] {
            assert!(!m.is_candidate(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn exclude_wins_over_include() {
        let m = matcher(&["**/*.controller.ts"], &["**/generated/**"]);
        assert!(m.is_candidate(Path::new("users/users.controller.ts")));
        assert!(!m.is_candidate(Path::new("generated/users.controller.ts")));
        assert!(!m.is_candidate(Path::new("a/generated/b/users.controller.ts")));
    }

    #[test]
    fn single_star_does_not_cross_directories() {
        let m = matcher(&["*.controller.ts"], &[]);
        assert!(m.is_candidate(Path::new("a.controller.ts")));
        assert!(!m.is_candidate(Path::new("sub/a.controller.ts")));
    }

    #[test]
    fn matching_is_case_sensitive() {
        let m = matcher(&["**/*.controller.ts"], &[]);
        assert!(!m.is_candidate(Path::new("Users.Controller.TS")));
    }

    #[test]
    fn directory_pruning_covers_patterns_that_only_match_children() {
        let m = matcher(&["**/*.ts"], &["**/dist/**", "legacy"]);
        assert!(m.is_excluded_dir(Path::new("dist")));
        assert!(m.is_excluded_dir(Path::new("a/dist")));
        assert!(m.is_excluded_dir(Path::new("legacy")));
        assert!(!m.is_excluded_dir(Path::new("distribution")));
        assert!(!m.is_excluded_dir(Path::new("a/legacy")));
    }

    #[test]
    fn partial_directory_excludes_do_not_prune_the_directory() {
        // `gen/*.ts` excludes only direct children; subdirectories must still
        // be walked because `gen/sub/x.ts` is not excluded.
        let m = matcher(&["**/*.ts"], &["gen/*.ts"]);
        assert!(!m.is_excluded_dir(Path::new("gen")));
        assert!(!m.is_candidate(Path::new("gen/a.ts")));
        assert!(m.is_candidate(Path::new("gen/sub/a.ts")));
    }

    #[test]
    fn typescript_extension_rules() {
        assert!(is_typescript_source(OsStr::new("a.controller.ts")));
        assert!(!is_typescript_source(OsStr::new("a.d.ts")));
        assert!(!is_typescript_source(OsStr::new("a.tsx")));
        assert!(!is_typescript_source(OsStr::new("a.js")));
        assert!(!is_typescript_source(OsStr::new("ts")));
        assert!(!is_typescript_source(OsStr::new(".ts.bak")));
    }

    #[test]
    fn invalid_patterns_are_reported_with_their_kind() {
        for (include, exclude, kind) in [
            (vec!["[abc"], vec![], "include"),
            (vec!["**/*.ts"], vec!["{a,b"], "exclude"),
        ] {
            let err = Matcher::new(&config(&include, &exclude)).err().unwrap();
            assert!(matches!(&err, Error::InvalidPattern { kind: k, .. } if *k == kind));
            assert!(err.to_string().contains(kind));
        }
    }
}
