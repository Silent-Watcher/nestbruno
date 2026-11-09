//! Filesystem containment validation for dependency resolution.
//!
//! The cross-file resolver reads dependency files it discovered through
//! import specifiers. Before any dependency is read, its project-relative
//! path is validated against the canonical project root:
//!
//! - the path must be relative and free of `.`/`..` components (the lexical
//!   escape check in [`super::module`] stays the first line of defense; this
//!   module re-checks defensively);
//! - no path component may be a symlink — this rejects symlinked targets,
//!   symlinked intermediate directories, and therefore symlink loops, which
//!   cannot be constructed without following a symlink;
//! - the final target must be a regular file: directories, FIFOs, sockets,
//!   devices and other special files are rejected. Reading a blocking special
//!   file would stall the analyzer, so existence alone is never sufficient.
//!
//! [`std::fs::canonicalize`] on the project root is performed once per
//! resolver; `std::fs::symlink_metadata` is used per component so symlinks
//! are seen as links, never followed.
//!
//! # Race conditions
//!
//! Validation and reading are separate operations: a concurrent process can
//! replace a path component between the two. Containment is defense in depth
//! for analyzing untrusted trees; it is not a guaranteed security boundary
//! and no claim is made that it eliminates time-of-check-to-time-of-use
//! races. NestBruno never executes anything it reads, which bounds the
//! impact of a lost race to reading swapped file content.

use std::fs;
use std::path::{Component, Path, PathBuf};

/// Why a dependency path was rejected before reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyPathError {
    /// The path is absolute, has a root/prefix component, or contains `.` or
    /// `..` components.
    NotRelative,
    /// A path component is a symlink (target, intermediate directory, or loop).
    Symlink(PathBuf),
    /// The final target exists but is not a regular file.
    NotARegularFile(PathBuf),
    /// The path does not exist (or is not accessible to this process).
    NotFound(PathBuf),
    /// The filesystem rejected a metadata query.
    Io(PathBuf),
}

impl std::fmt::Display for DependencyPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRelative => f.write_str("path is not project-relative"),
            Self::Symlink(p) => write!(f, "symlinked path component: {}", p.display()),
            Self::NotARegularFile(p) => {
                write!(f, "not a regular file: {}", p.display())
            }
            Self::NotFound(p) => write!(f, "path not found: {}", p.display()),
            Self::Io(p) => write!(f, "I/O error inspecting: {}", p.display()),
        }
    }
}

/// Validate `project_relative` against the canonical project root and return
/// the absolute path of a regular file that is reachable without following
/// any symlink.
pub fn validated_dependency_path(
    canonical_root: &Path,
    project_relative: &Path,
) -> Result<PathBuf, DependencyPathError> {
    // Defensive re-check of the lexical contract enforced by the module
    // resolver: relative, no dot components. `..` is always reported as
    // `Component::ParentDir`, so any non-Normal component is rejected.
    let mut components = project_relative.components().peekable();
    match components.peek() {
        Some(Component::Normal(_)) => {}
        _ => return Err(DependencyPathError::NotRelative),
    }
    for component in components {
        if !matches!(component, Component::Normal(_)) {
            return Err(DependencyPathError::NotRelative);
        }
    }

    // Walk from the canonical root, rejecting symlinks at every component.
    let mut current = canonical_root.to_path_buf();
    for part in project_relative.components() {
        let Component::Normal(part) = part else {
            return Err(DependencyPathError::NotRelative);
        };
        current.push(part);
        let meta = fs::symlink_metadata(&current).map_err(|e| io_error(&current, e))?;
        if meta.file_type().is_symlink() {
            return Err(DependencyPathError::Symlink(current));
        }
    }

    // Require a regular file: never follow what could block or is a tree.
    let meta = fs::symlink_metadata(&current).map_err(|e| io_error(&current, e))?;
    if !meta.file_type().is_file() {
        return Err(DependencyPathError::NotARegularFile(current));
    }
    Ok(current)
}

fn io_error(path: &Path, error: std::io::Error) -> DependencyPathError {
    if error.kind() == std::io::ErrorKind::NotFound {
        DependencyPathError::NotFound(path.to_path_buf())
    } else {
        DependencyPathError::Io(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn accepts_regular_file_within_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src/dto")).unwrap();
        fs::write(root.join("src/dto/x.ts"), "export class X {}").unwrap();

        let validated = validated_dependency_path(&root, Path::new("src/dto/x.ts")).unwrap();
        assert_eq!(validated, root.join("src/dto/x.ts"));
    }

    #[test]
    fn rejects_absolute_and_parent_paths() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(
            validated_dependency_path(&root, Path::new("/etc/passwd")),
            Err(DependencyPathError::NotRelative)
        );
        assert_eq!(
            validated_dependency_path(&root, Path::new("../outside.ts")),
            Err(DependencyPathError::NotRelative)
        );
        assert_eq!(
            validated_dependency_path(&root, Path::new("src/../secret.ts")),
            Err(DependencyPathError::NotRelative)
        );
        assert_eq!(
            validated_dependency_path(&root, Path::new("")),
            Err(DependencyPathError::NotRelative)
        );
    }

    #[test]
    fn reports_missing_files_as_not_found() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        assert_eq!(
            validated_dependency_path(&root, Path::new("src/missing.ts")),
            Err(DependencyPathError::NotFound(root.join("src/missing.ts")))
        );
    }

    #[test]
    fn rejects_directory_target() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src/dto.ts")).unwrap();
        assert_eq!(
            validated_dependency_path(&root, Path::new("src/dto.ts")),
            Err(DependencyPathError::NotARegularFile(
                root.join("src/dto.ts")
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_target_outside_root() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(outside.path().join("secret.ts"), "export class S {}").unwrap();
        symlink(outside.path().join("secret.ts"), root.join("src/leak.ts")).unwrap();

        assert_eq!(
            validated_dependency_path(&root, Path::new("src/leak.ts")),
            Err(DependencyPathError::Symlink(root.join("src/leak.ts")))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_intermediate_directory() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::write(outside.path().join("x.ts"), "export class X {}").unwrap();
        symlink(outside.path(), root.join("link")).unwrap();

        assert_eq!(
            validated_dependency_path(&root, Path::new("link/x.ts")),
            Err(DependencyPathError::Symlink(root.join("link")))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_loop() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        symlink(root.join("src/b.ts"), root.join("src/a.ts")).unwrap();
        symlink(root.join("src/a.ts"), root.join("src/b.ts")).unwrap();

        assert_eq!(
            validated_dependency_path(&root, Path::new("src/a.ts")),
            Err(DependencyPathError::Symlink(root.join("src/a.ts")))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_socket_special_file() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(root.join("src/pipe.ts")).unwrap();

        assert_eq!(
            validated_dependency_path(&root, Path::new("src/pipe.ts")),
            Err(DependencyPathError::NotARegularFile(
                root.join("src/pipe.ts")
            ))
        );
    }
}
