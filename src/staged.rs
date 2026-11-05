//! The Git staged-state boundary for `nestbruno sync --staged`.
//!
//! ```text
//! git index (+ HEAD) ─▶ staged changes (the gate) ─▶ staged sources (the state)
//!                                   │
//!        in-memory content ─▶ nest::analyze_sources ─▶ the same pipeline as sync
//! ```
//!
//! The *index* is the source of truth: staged synchronization analyzes the
//! project as the index represents it, never the working-tree version of a
//! staged file. When at least one staged change touches a relevant source
//! file, the desired route state is reconstructed from **all** relevant index
//! entries, not by patching a working-tree analysis — mixing the two could
//! produce a route model that corresponds to neither state. Unrelated staged
//! files (not matching `include`/`exclude`, outside the source directory) are
//! filtered before any content is read.
//!
//! Security: index paths and blob contents are untrusted input. Paths must
//! consist of normal components only and are mapped into the project before
//! anything else happens to them; blobs are size-capped and UTF-8-checked
//! exactly like source files. Nothing here spawns a process, runs a hook, or
//! writes to the repository: the index is only ever read.

use std::path::{Component, Path, PathBuf};

use git2::{Delta, DiffFindOptions, DiffOptions, Repository};

use crate::config::Config;
use crate::diag::{Diagnostic, Severity};
use crate::discovery::Matcher;
use crate::error::Error;
use crate::nest::extract::{MAX_SOURCE_BYTES, too_large};
use crate::project::Project;

/// One staged change to a relevant source file, in project-relative paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedChange {
    /// The project-relative path after the change (for deletions: the removed
    /// path).
    pub path: PathBuf,
    pub status: StagedStatus,
}

/// The staged status of a relevant file. Only statuses that libgit2 reports
/// reliably are represented; copies are reported as additions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    /// The path is in a merge conflict (no stage-0 entry); its content cannot
    /// be analyzed.
    Conflicted,
}

impl StagedStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::TypeChanged => "type-changed",
            Self::Conflicted => "conflicted",
        }
    }
}

/// The staged state relevant to synchronization.
#[derive(Debug, Default)]
pub struct Staged {
    /// Staged changes to relevant source files, sorted by path. Empty means
    /// nothing staged can affect the routes and the command is a no-op.
    pub changes: Vec<StagedChange>,
    /// Every relevant file of the index with its staged content
    /// (project-relative path, UTF-8 content). Sorted by path.
    pub sources: Vec<(PathBuf, String)>,
    /// Recoverable problems (unreadable blobs, conflicts, odd paths).
    pub diagnostics: Vec<Diagnostic>,
}

impl Staged {
    /// Whether anything staged can affect the routes.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Snapshot the staged state of `project`.
///
/// The index is read once; a concurrent `git add` during the rest of the run
/// is picked up by the next run, never mixed into this one. Fails fatally when
/// there is no repository at or above the project root, when the repository
/// itself cannot be read, or when the project root is not inside the
/// repository's worktree.
pub fn staged_sources(project: &Project, matcher: &Matcher) -> Result<Staged, Error> {
    let repo = Repository::discover(project.root()).map_err(|_| Error::GitNotARepository {
        start: project.root().to_owned(),
    })?;
    let workdir = repo
        .workdir()
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::GitRepository {
            repo: repo.path().to_owned(),
            message: "the repository is bare and has no worktree".to_owned(),
        })?;

    // The project root may be nested inside the worktree; index paths are
    // worktree-relative and must be mapped into the project first. Entries
    // outside the project belong to sibling projects and are ignored entirely.
    let root_in_workdir = project
        .root()
        .strip_prefix(&workdir)
        .map_err(|_| Error::GitRepository {
            repo: workdir.clone(),
            message: format!(
                "the project root {} is not inside this worktree",
                project.root().display()
            ),
        })?
        .to_owned();

    let index = repo.index().map_err(|err| Error::GitRepository {
        repo: workdir.clone(),
        message: err.to_string(),
    })?;

    let mut staged = Staged::default();
    collect_changes(
        &repo,
        &index,
        &root_in_workdir,
        &workdir,
        matcher,
        project.config(),
        &mut staged,
    )?;
    if staged.is_empty() {
        return Ok(staged);
    }
    // The desired state is the whole index, not just the changed files:
    // reconciliation needs the complete route set to detect stale requests.
    collect_sources(
        &repo,
        &index,
        &root_in_workdir,
        matcher,
        project.config(),
        &mut staged,
    );
    Ok(staged)
}

/// Diff HEAD (or the empty tree of a repository without commits) against the
/// index and record the deltas that touch relevant files.
fn collect_changes(
    repo: &Repository,
    index: &git2::Index,
    root_in_workdir: &Path,
    workdir: &Path,
    matcher: &Matcher,
    config: &Config,
    staged: &mut Staged,
) -> Result<(), Error> {
    let repository_error = |message: String| Error::GitRepository {
        repo: workdir.to_owned(),
        message,
    };

    // A repository without commits stages everything against an empty tree.
    let head_tree = match repo.head() {
        Ok(head) => Some(
            head.peel_to_tree()
                .map_err(|err| repository_error(format!("cannot read HEAD: {err}")))?,
        ),
        // An unborn branch (`git init` without commits) is the documented
        // empty-tree case.
        Err(err)
            if err.code() == git2::ErrorCode::NotFound
                || err.code() == git2::ErrorCode::UnbornBranch =>
        {
            None
        }
        Err(err) => return Err(repository_error(err.to_string())),
    };

    let mut options = DiffOptions::new();
    options.include_typechange(true);
    let mut diff = repo
        .diff_tree_to_index(head_tree.as_ref(), Some(index), Some(&mut options))
        .map_err(|err| repository_error(err.to_string()))?;
    diff.find_similar(Some(&mut DiffFindOptions::new().renames(true)))
        .map_err(|err| repository_error(err.to_string()))?;

    for delta in diff.deltas() {
        let status = match delta.status() {
            Delta::Added | Delta::Copied => StagedStatus::Added,
            Delta::Modified => StagedStatus::Modified,
            Delta::Deleted => StagedStatus::Deleted,
            Delta::Renamed => StagedStatus::Renamed,
            Delta::Typechange => StagedStatus::TypeChanged,
            Delta::Conflicted => StagedStatus::Conflicted,
            Delta::Unmodified | Delta::Ignored | Delta::Untracked | Delta::Unreadable => continue,
        };
        // Deletions have no new path; every other status names the new one.
        let workdir_relative = delta.new_file().path().or_else(|| delta.old_file().path());
        let Some(workdir_relative) = workdir_relative else {
            continue;
        };
        let Some(project_relative) = project_path(workdir_relative, root_in_workdir, staged) else {
            continue;
        };
        let Some(source_relative) = source_relative(&project_relative, config) else {
            continue;
        };
        if !matcher.is_relevant(&source_relative) {
            continue;
        }
        staged.changes.push(StagedChange {
            path: config.source.join(source_relative),
            status,
        });
    }
    staged.changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(())
}

/// Read the staged content of every relevant index entry.
fn collect_sources(
    repo: &Repository,
    index: &git2::Index,
    root_in_workdir: &Path,
    matcher: &Matcher,
    config: &Config,
    staged: &mut Staged,
) {
    for entry in index.iter() {
        // Index paths are raw bytes with `/` separators; non-UTF-8 ones cannot
        // be matched against the configuration. They are ignored (a verbose
        // note only), like non-UTF-8 file names elsewhere. They never make the
        // result "incomplete".
        let Ok(text) = std::str::from_utf8(&entry.path) else {
            staged.diagnostics.push(Diagnostic::new(
                Severity::Note,
                None,
                "an index entry has a non-UTF-8 path and is ignored",
            ));
            continue;
        };
        let workdir_relative = Path::new(text);
        let Some(project_relative) = project_path(workdir_relative, root_in_workdir, staged) else {
            continue;
        };
        let Some(source_relative) = source_relative(&project_relative, config) else {
            continue;
        };
        if !matcher.is_relevant(&source_relative) {
            continue;
        }
        let path = config.source.join(&source_relative);

        // Stage 0 exists only for non-conflicted entries.
        let entry = match index.get_path(workdir_relative, 0) {
            Some(entry) => entry,
            None => {
                staged.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    Some(path.clone()),
                    "unmerged path in the git index; the staged content is unknown, \
                     resolve the conflict and run again"
                        .to_owned(),
                ));
                continue;
            }
        };
        let blob = match repo.find_blob(entry.id) {
            Ok(blob) => blob,
            Err(err) => {
                staged.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    Some(path.clone()),
                    format!("cannot read the staged content: {err}"),
                ));
                continue;
            }
        };
        let bytes = blob.content();
        if bytes.len() > MAX_SOURCE_BYTES {
            staged.diagnostics.push(too_large(&path, bytes.len()));
            continue;
        }
        let content = match std::str::from_utf8(bytes) {
            Ok(content) => content.to_owned(),
            Err(_) => {
                staged.diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    Some(path.clone()),
                    "not valid UTF-8; skipped".to_owned(),
                ));
                continue;
            }
        };
        staged.sources.push((path, content));
    }
    staged.sources.sort_by(|a, b| a.0.cmp(&b.0));
}

/// Map a worktree-relative index path to a project-relative path. Returns
/// `None` — with a note for genuinely unsafe paths — when the entry lies
/// outside the project, or when the path is not plain.
fn project_path(
    workdir_relative: &Path,
    root_in_workdir: &Path,
    staged: &mut Staged,
) -> Option<PathBuf> {
    if !is_plain(workdir_relative) {
        staged.diagnostics.push(Diagnostic::new(
            Severity::Note,
            None,
            "the git index contains an unsafe path (absolute, `..`, or empty); ignored",
        ));
        return None;
    }
    // Strip the worktree→project prefix; entries elsewhere in the worktree
    // belong to sibling projects and are ignored entirely.
    if root_in_workdir.as_os_str().is_empty() {
        // The project root is the worktree root.
        return Some(workdir_relative.to_owned());
    }
    match workdir_relative.strip_prefix(root_in_workdir) {
        Ok(project_relative) if !project_relative.as_os_str().is_empty() => {
            Some(project_relative.to_owned())
        }
        _ => None,
    }
}

/// Whether the path is relative with normal components only.
fn is_plain(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.is_relative()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Restrict a project-relative path to the configured source directory and
/// return it relative to that directory (what `Matcher::is_relevant` expects).
/// Entries outside the source directory cannot affect the routes. The source
/// setting is matched by components, so `.` and `./src` behave as in discovery.
fn source_relative(project_relative: &Path, config: &Config) -> Option<PathBuf> {
    let source: Vec<Component<'_>> = config
        .source
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    let mut relative = project_relative
        .components()
        .filter(|component| !matches!(component, Component::CurDir));
    for component in source {
        if relative.next() != Some(component) {
            return None;
        }
    }
    let rest: PathBuf = relative.collect();
    if rest.as_os_str().is_empty() {
        None
    } else {
        Some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use tempfile::TempDir;

    const CONTROLLER: &str = "\
import { Controller, Get } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get()
  findAll() {}
}
";

    /// A git repository in a temp directory, without commits.
    struct Fixture {
        _dir: TempDir,
        repo: Repository,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let repo = Repository::init(dir.path()).expect("init repo");
            Self { _dir: dir, repo }
        }

        fn project(&self) -> Project {
            Project::open(self._dir.path()).expect("open project")
        }

        /// Stage `rel` (worktree-relative) with `content` without touching the
        /// working tree.
        fn stage(&self, rel: &str, content: &[u8]) {
            let id = self.repo.blob(content).expect("write blob");
            let mut index = self.repo.index().expect("read index");
            let entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: 0o100_644,
                uid: 0,
                gid: 0,
                file_size: u32::try_from(content.len()).expect("small blob"),
                id,
                flags: 0,
                flags_extended: 0,
                path: rel.as_bytes().to_vec(),
            };
            index.add_frombuffer(&entry, content).expect("stage buffer");
            index.write().expect("write index");
        }

        /// Commit the whole index, so HEAD exists.
        fn commit(&self, message: &str) {
            let mut index = self.repo.index().expect("read index");
            index.write().expect("write index");
            let tree_id = index.write_tree().expect("write tree");
            let tree = self.repo.find_tree(tree_id).expect("find tree");
            let signature =
                git2::Signature::now("test", "test@example.invalid").expect("signature");
            let parent = self
                .repo
                .head()
                .ok()
                .and_then(|head| head.peel_to_commit().ok());
            let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
            self.repo
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    message,
                    &tree,
                    &parents,
                )
                .expect("commit");
        }

        /// Remove `rel` from the index (a staged deletion).
        fn unstage(&self, rel: &Path) {
            let mut index = self.repo.index().expect("read index");
            index.remove_path(rel).expect("remove path");
            index.write().expect("write index");
        }
    }

    fn staged_of(fixture: &Fixture) -> Staged {
        let project = fixture.project();
        let matcher = Matcher::new(project.config()).expect("matcher");
        staged_sources(&project, &matcher).expect("staged sources")
    }

    #[test]
    fn a_repository_without_commits_stages_against_an_empty_tree() {
        let fixture = Fixture::new();
        fixture.stage("src/users.controller.ts", CONTROLLER.as_bytes());
        let staged = staged_of(&fixture);
        assert_eq!(staged.changes.len(), 1);
        assert_eq!(staged.changes[0].status, StagedStatus::Added);
        assert_eq!(staged.changes[0].path, Path::new("src/users.controller.ts"));
        assert_eq!(staged.sources.len(), 1);
        assert_eq!(staged.sources[0].0, Path::new("src/users.controller.ts"));
        assert_eq!(staged.sources[0].1, CONTROLLER);
        assert!(staged.diagnostics.is_empty());
    }

    #[test]
    fn no_repository_is_a_structured_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let project = Project::open(dir.path()).expect("open project");
        let matcher = Matcher::new(project.config()).expect("matcher");
        let error = staged_sources(&project, &matcher).expect_err("no repository");
        assert!(matches!(error, Error::GitNotARepository { .. }));
    }

    #[test]
    fn the_staged_version_is_used_not_the_working_tree() {
        let fixture = Fixture::new();
        fixture.stage("src/users.controller.ts", CONTROLLER.as_bytes());
        fixture.commit("initial");
        // The staged version differs from both HEAD and the working tree.
        fixture.stage(
            "src/users.controller.ts",
            CONTROLLER.replace("'users'", "'clients'").as_bytes(),
        );
        let _ = std::fs::write(
            fixture._dir.path().join("src/users.controller.ts"),
            CONTROLLER.replace("'users'", "'accounts'"),
        );

        let staged = staged_of(&fixture);
        assert_eq!(staged.changes.len(), 1);
        assert_eq!(staged.changes[0].status, StagedStatus::Modified);
        assert!(staged.sources[0].1.contains("'clients'"));
        assert!(!staged.sources[0].1.contains("'accounts'"));
    }

    #[test]
    fn an_unchanged_index_is_a_no_op() {
        let fixture = Fixture::new();
        fixture.stage("src/users.controller.ts", CONTROLLER.as_bytes());
        fixture.commit("initial");
        let staged = staged_of(&fixture);
        assert!(staged.is_empty());
        assert!(staged.sources.is_empty());
    }

    #[test]
    fn a_staged_deletion_is_reported_and_leaves_the_state() {
        let fixture = Fixture::new();
        fixture.stage("src/users.controller.ts", CONTROLLER.as_bytes());
        fixture.commit("initial");
        fixture.unstage(Path::new("src/users.controller.ts"));

        let staged = staged_of(&fixture);
        assert_eq!(staged.changes.len(), 1);
        assert_eq!(staged.changes[0].status, StagedStatus::Deleted);
        assert_eq!(staged.changes[0].path, Path::new("src/users.controller.ts"));
        // The route is gone from the desired state, which is what makes the
        // request of the deleted route stale.
        assert!(staged.sources.is_empty());
    }

    #[test]
    fn a_staged_rename_is_detected() {
        let fixture = Fixture::new();
        fixture.stage("src/old.controller.ts", CONTROLLER.as_bytes());
        fixture.commit("initial");
        fixture.unstage(Path::new("src/old.controller.ts"));
        fixture.stage("src/new.controller.ts", CONTROLLER.as_bytes());

        let staged = staged_of(&fixture);
        assert_eq!(staged.changes.len(), 1, "{:?}", staged.changes);
        assert_eq!(staged.changes[0].status, StagedStatus::Renamed);
        assert_eq!(staged.changes[0].path, Path::new("src/new.controller.ts"));
        // The desired state is the full index: exactly the new file.
        assert_eq!(staged.sources.len(), 1);
        assert_eq!(staged.sources[0].0, Path::new("src/new.controller.ts"));
    }

    #[test]
    fn unrelated_staged_files_are_filtered_before_content_is_read() {
        let fixture = Fixture::new();
        fixture.stage("README.md", b"# not a controller".as_slice());
        fixture.stage("node_modules/x/users.controller.ts", CONTROLLER.as_bytes());
        fixture.stage("other/thing.ts", b"export const x = 1;".as_slice());
        fixture.stage("src/helper.ts", b"export const y = 1;".as_slice());

        let staged = staged_of(&fixture);
        assert!(staged.is_empty(), "{:?}", staged.changes);
        assert!(staged.sources.is_empty());
        assert!(staged.diagnostics.is_empty());
    }

    #[test]
    fn entries_outside_the_source_directory_are_not_relevant() {
        let fixture = Fixture::new();
        fixture.stage("lib/users.controller.ts", CONTROLLER.as_bytes());
        let staged = staged_of(&fixture);
        assert!(staged.is_empty());
    }

    #[test]
    fn a_nested_project_root_is_mapped_from_the_worktree() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo = Repository::init(dir.path()).expect("init repo");
        let app = dir.path().join("app");
        std::fs::create_dir_all(app.join("src")).expect("create app/src");
        let project = Project::open(&app).expect("open project");

        let id = repo.blob(CONTROLLER.as_bytes()).expect("blob");
        let mut index = repo.index().expect("read index");
        let entry = git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100_644,
            uid: 0,
            gid: 0,
            file_size: CONTROLLER.len() as u32,
            id,
            flags: 0,
            flags_extended: 0,
            path: b"app/src/users.controller.ts".to_vec(),
        };
        index
            .add_frombuffer(&entry, CONTROLLER.as_bytes())
            .expect("stage");
        index.write().expect("write index");

        let matcher = Matcher::new(project.config()).expect("matcher");
        let staged = staged_sources(&project, &matcher).expect("staged");
        assert_eq!(staged.changes.len(), 1);
        assert_eq!(staged.changes[0].path, Path::new("src/users.controller.ts"));
    }

    #[test]
    fn non_utf8_content_is_an_error_diagnostic_and_incomplete() {
        let fixture = Fixture::new();
        let mut bytes = b"\xff\xfe".to_vec();
        bytes.extend_from_slice(CONTROLLER.as_bytes());
        fixture.stage("src/users.controller.ts", &bytes);

        let staged = staged_of(&fixture);
        assert_eq!(staged.changes.len(), 1);
        assert!(staged.sources.is_empty());
        assert!(
            staged
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("not valid UTF-8"))
        );
    }

    #[test]
    fn an_oversized_blob_is_rejected_like_a_file() {
        let fixture = Fixture::new();
        let mut content = b"@Controller()\nexport class HugeController {}\n".to_vec();
        content.resize(MAX_SOURCE_BYTES + 1, b' ');
        fixture.stage("src/huge.controller.ts", &content);

        let staged = staged_of(&fixture);
        assert!(staged.sources.is_empty());
        assert!(
            staged
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("too large to analyze"))
        );
    }

    #[test]
    fn source_mapping_understands_dot_and_prefixes() {
        let source = PathBuf::from("src");
        assert_eq!(
            source_relative(Path::new("src/users/x.ts"), &config_of(&source)),
            Some(PathBuf::from("users/x.ts"))
        );
        assert_eq!(
            source_relative(Path::new("docs/x.ts"), &config_of(&source)),
            None
        );
        assert_eq!(source_relative(Path::new("src"), &config_of(&source)), None);
        let dot = PathBuf::from(".");
        assert_eq!(
            source_relative(Path::new("users/x.ts"), &config_of(&dot)),
            Some(PathBuf::from("users/x.ts"))
        );
        assert_eq!(
            source_relative(Path::new("./users/x.ts"), &config_of(&dot)),
            Some(PathBuf::from("users/x.ts"))
        );
    }

    /// The mapping only reads `source`; a defaulted config with the value
    /// swapped in is enough.
    fn config_of(source: &Path) -> Config {
        Config {
            source: source.to_owned(),
            ..Config::default()
        }
    }

    use crate::config::Config;

    #[test]
    fn unsafe_index_paths_are_ignored() {
        assert!(is_plain(Path::new("src/x.ts")));
        assert!(!is_plain(Path::new("../x.ts")));
        assert!(!is_plain(Path::new("/abs/x.ts")));
        assert!(!is_plain(Path::new("")));
        assert!(!is_plain(Path::new("a/../b.ts")));
    }
}
