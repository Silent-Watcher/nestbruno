//! Source providers: where the cross-file resolver obtains dependency text.
//!
//! The resolver is written once against the [`SourceProvider`] boundary and
//! works unchanged against different backends:
//!
//! - [`DiskProvider`] — the working tree, with the full containment
//!   discipline (canonical root, symlink-free components, regular file).
//! - `staged::IndexProvider` — Git index blobs (mode-checked, snapshot-bound
//!   by object id), built in `crate::staged`.
//!
//! All provider reads are validated before content is handed to the parser;
//! a rejected or missing dependency is the caller's conservative
//! "unknown", never a guess and never a partially read file.

use std::fs;
use std::path::{Path, PathBuf};

use super::containment::{DependencyPathError, validated_dependency_path};

/// A source of dependency file content for cross-file resolution.
pub trait SourceProvider: Send {
    /// Whether `path` (project-relative) names a selectable regular file.
    /// Candidate selection in the module resolver consults this; symlinks,
    /// special files and unsupported entry modes answer `false`.
    fn exists(&self, path: &Path) -> bool;

    /// Validated read of `path` as UTF-8 text, or why it is unavailable.
    fn read(&self, path: &Path) -> Result<String, DependencyPathError>;
}

/// The working tree, behind the full containment discipline.
pub struct DiskProvider {
    /// Canonical project root; `None` when canonicalization failed, in which
    /// case every read is rejected conservatively.
    canonical_root: Option<PathBuf>,
}

impl DiskProvider {
    /// Provider for `project_root`. Returns the warning to emit when the root
    /// could not be canonicalized (cross-file resolution is then disabled).
    pub fn new(project_root: &Path) -> (Self, Option<String>) {
        match fs::canonicalize(project_root) {
            Ok(root) => (
                Self {
                    canonical_root: Some(root),
                },
                None,
            ),
            Err(_) => (
                Self {
                    canonical_root: None,
                },
                Some(
                    "cannot canonicalize project root; cross-file DTO resolution is disabled for this run"
                        .to_owned(),
                ),
            ),
        }
    }
}

impl SourceProvider for DiskProvider {
    fn exists(&self, path: &Path) -> bool {
        let Some(root) = &self.canonical_root else {
            return false;
        };
        // symlink_metadata never follows links: a symlinked candidate is
        // "selectable" so the read below can reject it with a precise
        // diagnostic instead of silently following it.
        fs::symlink_metadata(root.join(path)).is_ok()
    }

    fn read(&self, path: &Path) -> Result<String, DependencyPathError> {
        let root = self
            .canonical_root
            .as_ref()
            .ok_or_else(|| DependencyPathError::Io(path.to_path_buf()))?;
        let absolute = validated_dependency_path(root, path)?;
        fs::read_to_string(&absolute).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                DependencyPathError::NotFound(absolute.clone())
            } else {
                DependencyPathError::Io(absolute)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn disk_provider_reads_within_the_root_and_rejects_outside() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src/dto")).unwrap();
        fs::write(root.join("src/dto/x.ts"), "export class X {}").unwrap();
        let (provider, warning) = DiskProvider::new(dir.path());
        assert!(warning.is_none());

        assert!(provider.exists(Path::new("src/dto/x.ts")));
        assert!(!provider.exists(Path::new("src/dto/missing.ts")));
        assert_eq!(
            provider.read(Path::new("src/dto/x.ts")).unwrap(),
            "export class X {}"
        );
        assert_eq!(
            provider.read(Path::new("src/dto/missing.ts")),
            Err(DependencyPathError::NotFound(
                root.join("src/dto/missing.ts")
            ))
        );
    }

    #[test]
    fn disk_provider_without_a_canonical_root_rejects_everything() {
        let (provider, warning) = DiskProvider::new(Path::new("/nonexistent-root-∅"));
        assert!(warning.is_some());
        assert!(!provider.exists(Path::new("src/x.ts")));
        assert!(provider.read(Path::new("src/x.ts")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn disk_provider_never_follows_symlinks() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(outside.path().join("secret.ts"), "export class S {}").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.ts"), root.join("src/leak.ts"))
            .unwrap();
        let (provider, _) = DiskProvider::new(dir.path());
        assert!(
            matches!(
                provider.read(Path::new("src/leak.ts")),
                Err(DependencyPathError::Symlink(_))
            ),
            "a symlinked dependency is never read"
        );
    }
}
