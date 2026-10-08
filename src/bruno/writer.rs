//! Safe filesystem output for a generated collection.
//!
//! # Guarantees
//!
//! - **Containment.** Every path is built from the project root, the
//!   configured collection directory (validated in `config`) and single
//!   generated path components that are re-validated here. Nothing derived
//!   from a route string is ever joined into a path unchecked.
//! - **No symlinks.** The collection directory and each controller folder are
//!   walked component by component from the project root; any symlink aborts
//!   (collection directory) or fails that folder. Existing targets that are
//!   symlinks or non-files are never written through or replaced.
//! - **No silent overwrites.** An existing request file is left alone unless
//!   `force` is set, and `bruno.json` is never overwritten. A file whose
//!   content is already identical is reported as unchanged and not rewritten.
//! - **Atomic files.** Content is written completely to a temporary file in
//!   the target directory, flushed to disk, then moved into place (no-clobber
//!   for new files, replacing rename for `--force`). A crash or error leaves
//!   either the old file or the new file, never a partial one. Leftover
//!   `.nestbruno-*.tmp` files are possible after a crash; they are inert.
//! - **Failure scope.** Atomicity is per file, not per collection. A failure
//!   for one file or folder is recorded and the remaining files are still
//!   written; a failure to establish the collection directory aborts before
//!   anything is written.
//!
//! Known limits: the check-then-write sequence is not protected against a
//! concurrent process swapping a directory for a symlink in between, and the
//! containing directory is not `fsync`ed.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use tempfile::{Builder, NamedTempFile};

use super::GeneratedRequest;
use crate::diag::{Diagnostic, Severity};
use crate::error::Error;

/// Content of a newly created `bruno.json`.
pub const COLLECTION_FILE: &str = "bruno.json";
const COLLECTION_JSON: &str =
    "{\n  \"version\": \"1\",\n  \"name\": \"API\",\n  \"type\": \"collection\"\n}\n";

/// What happened to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Created,
    Overwritten,
    /// Already existed with identical content.
    Unchanged,
    /// Already existed with different content and `force` was not set.
    SkippedExisting,
    /// Could not be written; see the diagnostics.
    Failed,
}

/// Outcome of a write run.
#[derive(Debug, Default)]
pub struct WriteReport {
    /// `(path relative to the project root, status)` of the request files, in
    /// generation order.
    pub files: Vec<(PathBuf, FileStatus)>,
    /// Whether `bruno.json` was created by this run.
    pub collection_created: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl WriteReport {
    pub fn count(&self, status: FileStatus) -> usize {
        self.files.iter().filter(|(_, s)| *s == status).count()
    }

    /// Whether anything could not be written (a request file or `bruno.json`).
    pub fn has_failures(&self) -> bool {
        self.count(FileStatus::Failed) > 0
            || self
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
    }
}

/// Write `requests` (and `bruno.json` if missing) below `root`/`bruno_dir`.
///
/// `root` must be the canonical project root and `bruno_dir` the validated,
/// relative collection directory from the configuration.
pub fn write_collection(
    root: &Path,
    bruno_dir: &Path,
    requests: &[GeneratedRequest],
    force: bool,
) -> Result<WriteReport, Error> {
    let mut report = WriteReport::default();
    let bruno_components = relative_components(bruno_dir).ok_or_else(|| Error::UnsafeOutput {
        path: bruno_dir.to_owned(),
        reason: "the collection directory must be a relative path without '..'".to_owned(),
    })?;

    // Establishing the collection directory is fatal: without it nothing is safe.
    let collection = ensure_dir(root, &bruno_components).map_err(|reason| Error::UnsafeOutput {
        path: bruno_dir.to_owned(),
        reason,
    })?;

    let collection_rel = bruno_dir.join(COLLECTION_FILE);
    // An existing bruno.json is the user's; only its creation is reported.
    match write_file(&collection, COLLECTION_FILE, COLLECTION_JSON, false, true) {
        Ok(FileStatus::Created) => report.collection_created = true,
        Ok(_) => {}
        Err(reason) => report.diagnostics.push(Diagnostic::new(
            Severity::Error,
            Some(collection_rel),
            format!("not written: {reason}"),
        )),
    }

    // One directory check per folder, not per file.
    let mut folders: HashMap<&str, Result<PathBuf, String>> = HashMap::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for request in requests {
        let rel = bruno_dir.join(&request.folder).join(&request.file_name);

        if !seen.insert((
            request.folder.to_lowercase(),
            request.file_name.to_lowercase(),
        )) {
            fail(
                &mut report,
                rel,
                "two generated requests share this path (internal error)".to_owned(),
            );
            continue;
        }
        // Validate before creating anything, so a rejected request leaves no
        // empty folder behind.
        if let Err(reason) = check_component(&request.file_name) {
            fail(&mut report, rel, reason);
            continue;
        }
        let dir = folders
            .entry(request.folder.as_str())
            .or_insert_with(|| {
                check_component(&request.folder)?;
                ensure_dir(&collection, std::slice::from_ref(&request.folder))
            })
            .clone();
        let dir = match dir {
            Ok(dir) => dir,
            Err(reason) => {
                fail(&mut report, rel, reason);
                continue;
            }
        };

        match write_file(&dir, &request.file_name, &request.content, force, false) {
            Ok(status) => {
                if status == FileStatus::SkippedExisting {
                    report.diagnostics.push(Diagnostic::new(
                        Severity::Warning,
                        Some(rel.clone()),
                        "already exists with different content; left unchanged (use --force to replace it)",
                    ));
                }
                report.files.push((rel, status));
            }
            Err(reason) => fail(&mut report, rel, reason),
        }
    }
    Ok(report)
}

fn fail(report: &mut WriteReport, rel: PathBuf, reason: String) {
    report.diagnostics.push(Diagnostic::new(
        Severity::Error,
        Some(rel.clone()),
        format!("not written: {reason}"),
    ));
    report.files.push((rel, FileStatus::Failed));
}

/// The normal components of a relative path; `None` for anything else.
/// `.` components are dropped.
fn relative_components(path: &Path) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str()?;
                check_component(name).ok()?;
                out.push(name.to_owned());
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A single path component must be a plain, portable file name.
fn check_component(name: &str) -> Result<(), String> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.ends_with('.')
        || name.ends_with(' ')
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        });
    if bad {
        Err(format!("{name:?} is not a safe file name"))
    } else {
        Ok(())
    }
}

/// Create (if needed) and verify the directory `base/components...`, refusing
/// symlinks and non-directories at every level. Returns the directory path.
fn ensure_dir(base: &Path, components: &[String]) -> Result<PathBuf, String> {
    let mut path = base.to_owned();
    for component in components {
        check_component(component)?;
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symlink; refusing to write through it",
                    path.display()
                ));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(format!("{} exists and is not a directory", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(&path) {
                Ok(()) => {}
                // Created concurrently: accept only if it is a real directory.
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let ok = fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
                    if !ok {
                        return Err(format!(
                            "{} appeared and is not a directory",
                            path.display()
                        ));
                    }
                }
                Err(err) => return Err(format!("cannot create {}: {err}", path.display())),
            },
            Err(err) => return Err(format!("cannot inspect {}: {err}", path.display())),
        }
    }
    Ok(path)
}

/// Write one file according to the overwrite policy.
///
/// `never_replace` is for files that are user-owned once they exist
/// (`bruno.json`): a differing existing file is left alone even with `force`.
fn write_file(
    dir: &Path,
    name: &str,
    content: &str,
    force: bool,
    never_replace: bool,
) -> Result<FileStatus, String> {
    check_component(name)?;
    let target = dir.join(name);

    match fs::symlink_metadata(&target) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err(format!(
                    "{} exists and is a symlink or not a regular file; refusing to replace it",
                    target.display()
                ));
            }
            if same_content(&target, meta.len(), content)? {
                return Ok(FileStatus::Unchanged);
            }
            if !force || never_replace {
                return Ok(FileStatus::SkippedExisting);
            }
            let temp = write_temp(dir, content, Some(&meta.permissions()))?;
            temp.persist(&target)
                .map_err(|err| format!("cannot replace {}: {}", target.display(), err.error))?;
            Ok(FileStatus::Overwritten)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let temp = write_temp(dir, content, None)?;
            match temp.persist_noclobber(&target) {
                Ok(_) => Ok(FileStatus::Created),
                // Created by someone else since the check: never clobber.
                Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    Ok(FileStatus::SkippedExisting)
                }
                Err(err) => Err(format!("cannot create {}: {}", target.display(), err.error)),
            }
        }
        Err(err) => Err(format!("cannot inspect {}: {err}", target.display())),
    }
}

/// Whether the file at `path` (of `len` bytes) holds exactly `content`.
/// Compares bytes only; the file is not parsed.
fn same_content(path: &Path, len: u64, content: &str) -> Result<bool, String> {
    if len != content.len() as u64 {
        return Ok(false);
    }
    let mut existing = Vec::with_capacity(content.len());
    File::open(path)
        .and_then(|file| file.take(len + 1).read_to_end(&mut existing))
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    Ok(existing == content.as_bytes())
}

/// Write `content` completely to a fresh temporary file next to the target.
fn write_temp(
    dir: &Path,
    content: &str,
    permissions: Option<&fs::Permissions>,
) -> Result<NamedTempFile, String> {
    let mut temp = Builder::new()
        .prefix(".nestbruno-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .map_err(|err| format!("cannot create a temporary file in {}: {err}", dir.display()))?;
    temp.write_all(content.as_bytes())
        .and_then(|()| temp.flush())
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|err| format!("cannot write the temporary file: {err}"))?;

    // Temporary files are created private (0600); generated files are meant to
    // be shared like any other repository file.
    let permissions = permissions.cloned().or_else(default_permissions);
    if let Some(permissions) = permissions {
        fs::set_permissions(temp.path(), permissions)
            .map_err(|err| format!("cannot set permissions: {err}"))?;
    }
    Ok(temp)
}

#[cfg(unix)]
fn default_permissions() -> Option<fs::Permissions> {
    use std::os::unix::fs::PermissionsExt;
    Some(fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn default_permissions() -> Option<fs::Permissions> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bruno::identity::{ControllerIdentity, RouteIdentity};
    use crate::route::{HttpMethod, RoutePath};

    fn request(folder: &str, file: &str, content: &str) -> GeneratedRequest {
        GeneratedRequest {
            folder: folder.to_owned(),
            file_name: file.to_owned(),
            identity: RouteIdentity {
                controller: ControllerIdentity {
                    name: "C".into(),
                    file: PathBuf::from("a.ts"),
                },
                method: HttpMethod::Get,
                path: RoutePath::Static("/".into()),
                handler: "h".into(),
            },
            content: content.to_owned(),
        }
    }

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write(root: &Path, requests: &[GeneratedRequest], force: bool) -> WriteReport {
        let root = root.canonicalize().unwrap();
        write_collection(&root, Path::new("bruno"), requests, force).unwrap()
    }

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    fn leftovers(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_owned()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".nestbruno-")
                {
                    found.push(path);
                }
            }
        }
        found
    }

    #[test]
    fn creates_the_collection_folders_and_files() {
        let dir = project();
        let report = write(
            dir.path(),
            &[
                request("Users", "a.bru", "A\n"),
                request("Users", "b.bru", "B\n"),
                request("Auth", "c.bru", "C\n"),
            ],
            false,
        );
        assert_eq!(report.count(FileStatus::Created), 3, "{:?}", report.files);
        assert!(report.collection_created);
        assert!(report.diagnostics.is_empty());
        assert_eq!(read(dir.path(), "bruno/Users/a.bru"), "A\n");
        assert_eq!(read(dir.path(), "bruno/Auth/c.bru"), "C\n");
        assert!(read(dir.path(), "bruno/bruno.json").contains("\"type\": \"collection\""));
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_second_identical_run_changes_nothing() {
        let dir = project();
        let requests = [request("Users", "a.bru", "A\n")];
        write(dir.path(), &requests, false);
        let before = fs::metadata(dir.path().join("bruno/Users/a.bru"))
            .unwrap()
            .modified()
            .unwrap();
        let report = write(dir.path(), &requests, false);
        assert_eq!(report.count(FileStatus::Unchanged), 1);
        assert_eq!(report.count(FileStatus::Created), 0);
        assert!(report.diagnostics.is_empty());
        let after = fs::metadata(dir.path().join("bruno/Users/a.bru"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "an identical file must not be rewritten");
    }

    #[test]
    fn existing_files_with_different_content_are_not_overwritten_by_default() {
        let dir = project();
        write(
            dir.path(),
            &[request("Users", "a.bru", "generated\n")],
            false,
        );
        fs::write(dir.path().join("bruno/Users/a.bru"), "user edits\n").unwrap();

        let report = write(
            dir.path(),
            &[request("Users", "a.bru", "generated\n")],
            false,
        );
        assert_eq!(report.count(FileStatus::SkippedExisting), 1);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].severity, Severity::Warning);
        assert!(report.diagnostics[0].message.contains("--force"));
        assert_eq!(read(dir.path(), "bruno/Users/a.bru"), "user edits\n");
    }

    #[test]
    fn force_replaces_existing_files() {
        let dir = project();
        fs::create_dir_all(dir.path().join("bruno/Users")).unwrap();
        fs::write(dir.path().join("bruno/Users/a.bru"), "old\n").unwrap();
        let report = write(dir.path(), &[request("Users", "a.bru", "new\n")], true);
        assert_eq!(report.count(FileStatus::Overwritten), 1);
        assert_eq!(read(dir.path(), "bruno/Users/a.bru"), "new\n");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn bruno_json_is_never_overwritten_even_with_force() {
        let dir = project();
        fs::create_dir(dir.path().join("bruno")).unwrap();
        fs::write(dir.path().join("bruno/bruno.json"), "{\"name\": \"mine\"}").unwrap();
        let report = write(dir.path(), &[request("Users", "a.bru", "x\n")], true);
        assert_eq!(read(dir.path(), "bruno/bruno.json"), "{\"name\": \"mine\"}");
        assert!(report.diagnostics.is_empty());
        assert_eq!(report.files.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn new_files_are_world_readable_and_replacements_keep_their_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = project();
        write(dir.path(), &[request("Users", "a.bru", "x\n")], false);
        let mode = |rel: &str| {
            fs::metadata(dir.path().join(rel))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("bruno/Users/a.bru"), 0o644);

        fs::set_permissions(
            dir.path().join("bruno/Users/a.bru"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        write(dir.path(), &[request("Users", "a.bru", "y\n")], true);
        assert_eq!(mode("bruno/Users/a.bru"), 0o600);
    }

    #[cfg(unix)]
    mod symlinks {
        use super::*;
        use std::os::unix::fs::symlink;

        #[test]
        fn a_symlinked_collection_directory_is_refused() {
            let dir = project();
            let outside = project();
            symlink(outside.path(), dir.path().join("bruno")).unwrap();
            let root = dir.path().canonicalize().unwrap();
            let err = write_collection(
                &root,
                Path::new("bruno"),
                &[request("U", "a.bru", "x\n")],
                true,
            )
            .unwrap_err();
            assert!(matches!(err, Error::UnsafeOutput { .. }), "{err}");
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        }

        #[test]
        fn a_symlink_in_the_middle_of_the_collection_path_is_refused() {
            let dir = project();
            let outside = project();
            fs::create_dir(dir.path().join("docs")).unwrap();
            symlink(outside.path(), dir.path().join("docs/api")).unwrap();
            let root = dir.path().canonicalize().unwrap();
            let err = write_collection(
                &root,
                Path::new("docs/api"),
                &[request("U", "a.bru", "x\n")],
                false,
            )
            .unwrap_err();
            assert!(matches!(err, Error::UnsafeOutput { .. }));
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        }

        #[test]
        fn a_symlinked_folder_fails_only_its_own_files() {
            let dir = project();
            let outside = project();
            fs::create_dir(dir.path().join("bruno")).unwrap();
            symlink(outside.path(), dir.path().join("bruno/Users")).unwrap();
            let report = write(
                dir.path(),
                &[
                    request("Users", "a.bru", "x\n"),
                    request("Auth", "b.bru", "y\n"),
                ],
                true,
            );
            assert_eq!(report.count(FileStatus::Failed), 1);
            assert_eq!(report.count(FileStatus::Created), 1); // Auth/b.bru
            assert!(report.collection_created);
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
            assert!(report.diagnostics[0].message.contains("symlink"));
        }

        #[test]
        fn existing_symlink_targets_are_never_written_through() {
            let dir = project();
            let outside = project();
            let victim = outside.path().join("victim.txt");
            fs::write(&victim, "precious").unwrap();
            fs::create_dir_all(dir.path().join("bruno/Users")).unwrap();
            symlink(&victim, dir.path().join("bruno/Users/a.bru")).unwrap();
            for force in [false, true] {
                let report = write(dir.path(), &[request("Users", "a.bru", "x\n")], force);
                assert_eq!(report.count(FileStatus::Failed), 1, "force={force}");
                assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
            }
        }

        #[test]
        fn dangling_symlink_targets_are_not_created_through() {
            let dir = project();
            let outside = project();
            let target = outside.path().join("new.txt");
            fs::create_dir_all(dir.path().join("bruno/Users")).unwrap();
            symlink(&target, dir.path().join("bruno/Users/a.bru")).unwrap();
            let report = write(dir.path(), &[request("Users", "a.bru", "x\n")], true);
            assert_eq!(report.count(FileStatus::Failed), 1);
            assert!(!target.exists());
        }
    }

    #[test]
    fn a_directory_in_place_of_a_file_is_not_replaced() {
        let dir = project();
        fs::create_dir_all(dir.path().join("bruno/Users/a.bru")).unwrap();
        let report = write(dir.path(), &[request("Users", "a.bru", "x\n")], true);
        assert_eq!(report.count(FileStatus::Failed), 1);
        assert!(dir.path().join("bruno/Users/a.bru").is_dir());
    }

    #[test]
    fn a_file_in_place_of_a_folder_fails_only_that_folder() {
        let dir = project();
        fs::create_dir(dir.path().join("bruno")).unwrap();
        fs::write(dir.path().join("bruno/Users"), "i am a file").unwrap();
        let report = write(
            dir.path(),
            &[
                request("Users", "a.bru", "x\n"),
                request("Auth", "b.bru", "y\n"),
            ],
            false,
        );
        assert_eq!(report.count(FileStatus::Failed), 1);
        assert_eq!(read(dir.path(), "bruno/Auth/b.bru"), "y\n");
        assert_eq!(read(dir.path(), "bruno/Users"), "i am a file");
    }

    #[test]
    fn unsafe_components_are_rejected_and_nothing_escapes() {
        let dir = project();
        let outer = dir.path().join("outer");
        fs::create_dir(&outer).unwrap();
        let bad = [
            ("..", "a.bru"),
            (".", "a.bru"),
            ("", "a.bru"),
            ("a/b", "c.bru"),
            ("a\\b", "c.bru"),
            ("U", "../../escape.bru"),
            ("U", "/abs.bru"),
            ("U", "C:evil.bru"),
            ("U", "a\nb.bru"),
            ("U", ""),
            ("CON:stream", "a.bru"),
            ("trailing.", "a.bru"),
            ("U", "x\0.bru"),
        ];
        for (folder, file) in bad {
            let root = outer.canonicalize().unwrap();
            let report = write_collection(
                &root,
                Path::new("bruno"),
                &[request(folder, file, "x\n")],
                true,
            )
            .unwrap();
            assert_eq!(report.count(FileStatus::Failed), 1, "{folder:?}/{file:?}");
        }
        // Only the collection file may have been created.
        let mut created: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        created.sort();
        assert_eq!(created, ["outer"]);
        let inside: Vec<_> = fs::read_dir(outer.join("bruno"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(inside, ["bruno.json"]);
    }

    #[test]
    fn unsafe_collection_directories_are_fatal() {
        let dir = project();
        let root = dir.path().canonicalize().unwrap();
        for bad in ["../x", "/abs", "a/../../b", "", "."] {
            let err = write_collection(&root, Path::new(bad), &[], false).unwrap_err();
            assert!(matches!(err, Error::UnsafeOutput { .. }), "{bad:?}");
        }
    }

    #[test]
    fn duplicate_destinations_are_detected_even_case_insensitively() {
        let dir = project();
        let report = write(
            dir.path(),
            &[
                request("Users", "a.bru", "1\n"),
                request("users", "A.bru", "2\n"),
            ],
            true,
        );
        assert_eq!(report.count(FileStatus::Failed), 1);
        assert_eq!(read(dir.path(), "bruno/Users/a.bru"), "1\n");
    }

    #[test]
    fn nested_collection_directories_are_created() {
        let dir = project();
        let root = dir.path().canonicalize().unwrap();
        write_collection(
            &root,
            Path::new("docs/api/bruno"),
            &[request("U", "a.bru", "x\n")],
            false,
        )
        .unwrap();
        assert_eq!(read(dir.path(), "docs/api/bruno/U/a.bru"), "x\n");
    }

    #[cfg(unix)]
    #[test]
    fn write_failures_leave_the_old_file_and_no_temp_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = project();
        write(dir.path(), &[request("U", "a.bru", "old\n")], false);
        let folder = dir.path().join("bruno/U");
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores directory modes; nothing to observe then.
        let writable = File::create(folder.join(".probe")).is_ok();
        if writable {
            fs::remove_file(folder.join(".probe")).unwrap();
        } else {
            let report = write(dir.path(), &[request("U", "a.bru", "new\n")], true);
            assert_eq!(report.count(FileStatus::Failed), 1);
            assert_eq!(read(dir.path(), "bruno/U/a.bru"), "old\n");
        }
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(leftovers(dir.path()).is_empty());
    }
}
