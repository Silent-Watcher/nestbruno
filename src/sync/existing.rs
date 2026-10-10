//! Discovery and classification of the `.bru` files already in the collection.
//!
//! Only the configured collection directory is inspected. Symlinks are never
//! followed, `.git` and `node_modules` are not entered, results are sorted,
//! each file is read at most once and read in bounded size.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::bruno::identity::content_hash;
use crate::bruno::stamp::{self, Parsed, Stamp};
use crate::bruno::writer::{existing_dir, relative_components};
use crate::diag::{Diagnostic, Severity};
use crate::error::Error;

/// Larger `.bru` files are not read (they are classified invalid).
pub const MAX_BRU_BYTES: u64 = 4 * 1024 * 1024;
/// Nesting limit of the collection directory walk.
const MAX_DEPTH: usize = 32;

/// What an existing file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Has a valid stamp. `modified` is true if its content no longer matches
    /// what was generated.
    Generated { stamp: Stamp, modified: bool },
    /// A request or other `.bru` file that NestBruno did not generate.
    Unknown,
    /// Cannot be understood (reason); never touched.
    Invalid(String),
    /// Has a NestBruno line that cannot be trusted (reason); never touched.
    InvalidMetadata(String),
}

/// One existing `.bru` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingFile {
    /// Relative to the collection directory.
    pub path: PathBuf,
    pub kind: Kind,
    /// Hash of the whole file as read (for the pre-write re-check).
    pub full_hash: u64,
    /// The text, kept only when needed later (stamped files and files that
    /// sit where a new request would be created).
    pub text: Option<String>,
}

/// All existing files, sorted by path.
#[derive(Debug, Default)]
pub struct Existing {
    pub files: Vec<ExistingFile>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Existing {
    /// Whether any file could not be read.
    pub fn is_incomplete(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

/// Classify the text of one file.
pub fn classify(text: &str) -> Kind {
    if text.contains('\0') {
        return Kind::Invalid("binary content".to_owned());
    }
    match stamp::parse(text) {
        Parsed::Unstamped => Kind::Unknown,
        Parsed::Malformed(reason) => Kind::InvalidMetadata(reason),
        Parsed::Stamped(stamp) => Kind::Generated {
            modified: stamp::normalized_hash(text) != stamp.hash,
            stamp,
        },
    }
}

/// Load the collection `root/bruno_dir`. A missing directory is an empty
/// collection. `wanted` holds lower-cased relative paths (`Folder/file.bru`)
/// whose text must be kept even for unstamped files.
pub fn load(root: &Path, bruno_dir: &Path, wanted: &HashSet<String>) -> Result<Existing, Error> {
    let components = relative_components(bruno_dir).ok_or_else(|| Error::UnsafeOutput {
        path: bruno_dir.to_owned(),
        reason: "the collection directory must be a relative path without '..'".to_owned(),
    })?;
    let collection = match existing_dir(root, &components) {
        Ok(path) => path,
        // Not there yet (or a parent is missing): nothing exists.
        Err(_) if !exists_at_all(root, &components) => return Ok(Existing::default()),
        Err(reason) => {
            return Err(Error::UnsafeOutput {
                path: bruno_dir.to_owned(),
                reason,
            });
        }
    };

    let mut existing = Existing::default();
    let mut pending = vec![(collection.clone(), PathBuf::new(), 0_usize)];
    while let Some((dir, relative, depth)) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                existing.diagnostics.push(problem(
                    Severity::Error,
                    bruno_dir,
                    &relative,
                    format!("cannot read directory: {err}"),
                ));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    existing.diagnostics.push(problem(
                        Severity::Error,
                        bruno_dir,
                        &relative,
                        format!("cannot read directory entry: {err}"),
                    ));
                    continue;
                }
            };
            let name = entry.file_name();
            let rel = relative.join(&name);
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(err) => {
                    existing.diagnostics.push(problem(
                        Severity::Error,
                        bruno_dir,
                        &rel,
                        format!("cannot determine file type: {err}"),
                    ));
                    continue;
                }
            };
            if file_type.is_symlink() {
                existing.diagnostics.push(problem(
                    Severity::Note,
                    bruno_dir,
                    &rel,
                    "symlink not followed".to_owned(),
                ));
            } else if file_type.is_dir() {
                if name == ".git" || name == "node_modules" {
                    continue;
                }
                if depth + 1 > MAX_DEPTH {
                    existing.diagnostics.push(problem(
                        Severity::Error,
                        bruno_dir,
                        &rel,
                        format!("directory nesting exceeds {MAX_DEPTH} levels; not entered"),
                    ));
                    continue;
                }
                pending.push((entry.path(), rel, depth + 1));
            } else if file_type.is_file()
                && Path::new(&name).extension().is_some_and(|e| e == "bru")
            {
                existing.files.push(read_one(
                    &entry.path(),
                    rel,
                    bruno_dir,
                    wanted,
                    &mut existing.diagnostics,
                ));
            }
        }
    }
    existing.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(existing)
}

fn exists_at_all(root: &Path, components: &[String]) -> bool {
    let mut path = root.to_owned();
    for component in components {
        path.push(component);
        if fs::symlink_metadata(&path).is_err() {
            return false;
        }
    }
    true
}

fn read_one(
    path: &Path,
    rel: PathBuf,
    bruno_dir: &Path,
    wanted: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) -> ExistingFile {
    let invalid = |reason: String, full_hash: u64| ExistingFile {
        path: rel.clone(),
        kind: Kind::Invalid(reason),
        full_hash,
        text: None,
    };

    let mut bytes = Vec::new();
    let read =
        File::open(path).and_then(|file| file.take(MAX_BRU_BYTES + 1).read_to_end(&mut bytes));
    if let Err(err) = read {
        diagnostics.push(problem(
            Severity::Error,
            bruno_dir,
            &rel,
            format!("cannot read file: {err}"),
        ));
        return invalid("unreadable".to_owned(), 0);
    }
    if bytes.len() as u64 > MAX_BRU_BYTES {
        diagnostics.push(problem(
            Severity::Warning,
            bruno_dir,
            &rel,
            format!("larger than {MAX_BRU_BYTES} bytes; left untouched"),
        ));
        return invalid("too large".to_owned(), 0);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        diagnostics.push(problem(
            Severity::Warning,
            bruno_dir,
            &rel,
            "not valid UTF-8; left untouched".to_owned(),
        ));
        return invalid("not valid UTF-8".to_owned(), 0);
    };

    let kind = classify(&text);
    if let Kind::Invalid(reason) = &kind {
        diagnostics.push(problem(
            Severity::Warning,
            bruno_dir,
            &rel,
            format!("{reason}; left untouched"),
        ));
    }
    let keep = matches!(kind, Kind::Generated { .. })
        || wanted.contains(&rel.to_string_lossy().replace('\\', "/").to_lowercase());
    ExistingFile {
        path: rel,
        full_hash: content_hash(&text),
        kind,
        text: keep.then_some(text),
    }
}

fn problem(severity: Severity, bruno_dir: &Path, rel: &Path, message: String) -> Diagnostic {
    Diagnostic::new(severity, Some(bruno_dir.join(rel)), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bruno::stamp;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn load_all(root: &Path) -> Existing {
        load(root, Path::new("bruno"), &HashSet::new()).unwrap()
    }

    fn names(existing: &Existing) -> Vec<String> {
        existing
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    fn generated_text() -> String {
        use crate::bruno::identity::{ControllerIdentity, RouteIdentity};
        use crate::route::{HttpMethod, RoutePath};
        let id = RouteIdentity {
            controller: ControllerIdentity {
                name: "C".into(),
                file: "a.ts".into(),
            },
            method: HttpMethod::Get,
            path: RoutePath::Static("/x".into()),
            handler: "h".into(),
        };
        let base = "meta {\n  name: GET /x\n  type: http\n  seq: 1\n}\n\nget {\n  url: {{baseUrl}}/x\n  body: none\n  auth: inherit\n}\n\ndocs {\n  NestJS: C.h\n}\n";
        stamp::stamp(base, &id).unwrap()
    }

    #[test]
    fn classification_covers_every_kind() {
        let text = generated_text();
        assert!(matches!(
            classify(&text),
            Kind::Generated {
                modified: false,
                ..
            }
        ));
        assert!(matches!(
            classify(&text.replace("auth: inherit", "auth: none")),
            Kind::Generated { modified: true, .. }
        ));
        assert_eq!(classify("meta {\n  name: x\n}\n"), Kind::Unknown);
        assert_eq!(classify(""), Kind::Unknown);
        assert!(matches!(classify("a\0b"), Kind::Invalid(_)));
        assert!(matches!(
            classify("docs {\n  NestBruno: nope\n}\n"),
            Kind::InvalidMetadata(_)
        ));
    }

    #[test]
    fn a_missing_collection_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let existing = load_all(dir.path());
        assert!(existing.files.is_empty() && existing.diagnostics.is_empty());
        fs::create_dir(dir.path().join("bruno")).unwrap();
        assert!(load_all(dir.path()).files.is_empty());
    }

    #[test]
    fn only_bru_files_are_found_recursively_and_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for rel in [
            "bruno/z.bru",
            "bruno/B/b.bru",
            "bruno/A/a.bru",
            "bruno/A/deep/er/x.bru",
            "bruno/notes.txt",
            "bruno/bruno.json",
            "bruno/A/a.bru.bak",
            "bruno/UP.BRU",
        ] {
            write(dir.path(), rel, "x");
        }
        assert_eq!(
            names(&load_all(dir.path())),
            ["A/a.bru", "A/deep/er/x.bru", "B/b.bru", "z.bru"]
        );
    }

    #[test]
    fn node_modules_and_git_are_not_entered() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bruno/node_modules/pkg/x.bru", "x");
        write(dir.path(), "bruno/.git/y.bru", "x");
        write(dir.path(), "bruno/ok.bru", "x");
        assert_eq!(names(&load_all(dir.path())), ["ok.bru"]);
    }

    #[test]
    fn text_is_kept_for_stamped_and_wanted_files_only() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bruno/Gen/g.bru", &generated_text());
        write(dir.path(), "bruno/Other/o.bru", "meta {\n}\n");
        write(dir.path(), "bruno/Want/w.bru", "meta {\n}\n");
        let wanted: HashSet<String> = ["want/w.bru".to_owned()].into();
        let existing = load(dir.path(), Path::new("bruno"), &wanted).unwrap();
        let kept: Vec<_> = existing
            .files
            .iter()
            .map(|f| {
                (
                    f.path.to_string_lossy().replace('\\', "/"),
                    f.text.is_some(),
                )
            })
            .collect();
        assert_eq!(
            kept,
            [
                ("Gen/g.bru".into(), true),
                ("Other/o.bru".into(), false),
                ("Want/w.bru".into(), true)
            ]
        );
    }

    #[test]
    fn invalid_files_are_classified_and_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bruno")).unwrap();
        fs::write(dir.path().join("bruno/bin.bru"), [0xff, 0xfe, 0x00]).unwrap();
        fs::write(dir.path().join("bruno/nul.bru"), "a\0b").unwrap();
        fs::write(
            dir.path().join("bruno/huge.bru"),
            vec![b'x'; MAX_BRU_BYTES as usize + 1],
        )
        .unwrap();
        write(dir.path(), "bruno/good.bru", "meta {\n}\n");
        let existing = load_all(dir.path());
        let kinds: Vec<_> = existing
            .files
            .iter()
            .map(|f| {
                (
                    f.path.to_str().unwrap().to_owned(),
                    matches!(f.kind, Kind::Invalid(_)),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("bin.bru".into(), true),
                ("good.bru".into(), false),
                ("huge.bru".into(), true),
                ("nul.bru".into(), true)
            ]
        );
        assert_eq!(existing.diagnostics.len(), 3);
        assert!(
            !existing.is_incomplete(),
            "invalid content is not an unreadable file"
        );
    }

    #[test]
    fn identical_files_have_identical_hashes_and_different_files_do_not() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bruno/a.bru", "same");
        write(dir.path(), "bruno/b.bru", "same");
        write(dir.path(), "bruno/c.bru", "different");
        let e = load_all(dir.path());
        assert_eq!(e.files[0].full_hash, e.files[1].full_hash);
        assert_ne!(e.files[0].full_hash, e.files[2].full_hash);
    }

    #[cfg(unix)]
    mod unix {
        use std::os::unix::fs::{PermissionsExt, symlink};

        use super::*;

        #[test]
        fn symlinks_inside_the_collection_are_not_followed() {
            let dir = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            write(outside.path(), "secret.bru", "outside");
            write(dir.path(), "bruno/real.bru", "x");
            symlink(outside.path(), dir.path().join("bruno/linked-dir")).unwrap();
            symlink(
                outside.path().join("secret.bru"),
                dir.path().join("bruno/linked.bru"),
            )
            .unwrap();
            let existing = load_all(dir.path());
            assert_eq!(names(&existing), ["real.bru"]);
            assert_eq!(existing.diagnostics.len(), 2);
            assert!(
                existing
                    .diagnostics
                    .iter()
                    .all(|d| d.severity == Severity::Note)
            );
        }

        #[test]
        fn a_symlinked_collection_directory_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            symlink(outside.path(), dir.path().join("bruno")).unwrap();
            let err = load(dir.path(), Path::new("bruno"), &HashSet::new()).unwrap_err();
            assert!(matches!(err, Error::UnsafeOutput { .. }), "{err}");
        }

        #[test]
        fn unreadable_files_make_the_result_incomplete() {
            let dir = tempfile::tempdir().unwrap();
            write(dir.path(), "bruno/locked.bru", "x");
            let path = dir.path().join("bruno/locked.bru");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
            if fs::read(&path).is_ok() {
                return; // running as root: nothing to observe
            }
            let existing = load_all(dir.path());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(existing.is_incomplete());
            assert!(matches!(existing.files[0].kind, Kind::Invalid(_)));
        }
    }
}
