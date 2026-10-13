//! Incremental state for long-running modes (watch).
//!
//! A [`Workspace`] holds what a full `sync` would compute from scratch, so that
//! a change to one source file only costs what that change requires:
//!
//! ```text
//! dirty paths ─▶ re-stat / re-read / re-parse only those files
//!             ─▶ replace their contribution to the route IR
//!             ─▶ regenerate only the affected controllers (cached per controller)
//!             ─▶ reconcile (the same pure rules as `sync`) against the in-memory
//!                index of the collection
//!             ─▶ write only the planned changes
//! ```
//!
//! # Principles
//!
//! - **Events are hints, the filesystem is the truth.** A path is merely
//!   *dirty*; processing looks at the file as it is now. Duplicate, reordered
//!   or partial event sequences (editor save dances, renames) cannot
//!   misrepresent the state.
//! - **Incremental ≡ full.** After any sequence of changes the collection is
//!   what a fresh `sync` of the final source state would produce (modulo file
//!   names kept by in-place updates and the user-owned `seq`). A property test
//!   checks this with random histories.
//! - **Never act on a partial view.** A file that fails to parse keeps its last
//!   good routes, and while any file is in that state nothing is deleted.
//! - **No stale collection index.** The index of existing `.bru` files is kept
//!   in memory and updated from our own writes. Every replace/delete re-verifies
//!   the file's hash on disk first; if the user changed it meanwhile, the file
//!   is re-read and the plan recomputed once.
//!
//! The reconciliation pass itself is a linear, in-memory pass over all routes.
//! Restricting it to affected routes would complicate matching (the rules look
//! at uniqueness across the whole set); it is only worth doing if measurements
//! (Phase 10) show it matters.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::bruno::identity::{RouteIdentity, content_hash};
use crate::bruno::{GeneratedRequest, Generation, folder_names, generate_controller};
use crate::config::Config;
use crate::diag::{Diagnostic, Severity};
use crate::discovery::{ControllerFile, Matcher, discover_controllers, discover_in};
use crate::error::Error;
use crate::nest::analyze_each;
use crate::project::Project;
use crate::route::Controller;
use crate::sync::existing::{self, Existing, ExistingFile, classify};
use crate::sync::{Action, Desired, Plan, PlanEntry, SyncResult, apply, reconcile};

/// How many distinct standing diagnostics are remembered to avoid repeating them.
const MAX_REMEMBERED_NOTES: usize = 1_000;

/// What one synchronization step did.
#[derive(Debug, Default)]
pub struct Delta {
    /// Source files whose state was re-examined (project-relative, sorted).
    pub changed_files: Vec<PathBuf>,
    /// Number of routes (distinct handlers) that were added, removed or changed
    /// (identity or parameters) in the changed files.
    pub routes_affected: usize,
    /// What was created, updated and deleted, and the conflicts that are *new*
    /// since the previous step (a standing conflict is reported once).
    pub result: SyncResult,
    /// Problems of the changed files and of this step.
    pub diagnostics: Vec<Diagnostic>,
    /// Whether everything was recomputed from disk.
    pub full: bool,
}

/// The cached analysis of one source file.
struct FileState {
    /// Controllers from the last successful analysis.
    controllers: Vec<Controller>,
    /// The last analysis could not read or parse the file; `controllers` are
    /// from before that.
    failed: bool,
    /// Generation of each controller, with the folder it was generated for.
    /// `None` until needed and after the file changes.
    parts: Vec<Option<Part>>,
}

struct Part {
    folder: String,
    generation: Generation,
}

/// Incremental synchronization state. See the module documentation.
pub struct Workspace {
    project: Project,
    matcher: Matcher,
    /// `config.source` without `.` components: the project-relative prefix of
    /// every source path.
    source_prefix: PathBuf,
    /// The collection directory, project-relative. Never a source, and never
    /// watched, even when it lies inside the source tree (`source = "."`).
    bruno_prefix: PathBuf,
    files: BTreeMap<PathBuf, FileState>,
    /// Errors met while walking the source tree.
    walk_incomplete: bool,
    collection: Existing,
    by_path: HashMap<String, usize>,
    reported_conflicts: HashSet<(PathBuf, String)>,
    reported_notes: HashSet<String>,
}

impl Workspace {
    /// Analyze the whole project, load the collection and bring it in sync.
    pub fn start(project: Project) -> Result<(Self, Delta), Error> {
        let matcher = Matcher::new(project.config())?;
        let source_prefix = clean_prefix(&project.config().source);
        let bruno_prefix = clean_prefix(&project.config().bruno);
        let mut workspace = Self {
            project,
            matcher,
            source_prefix,
            bruno_prefix,
            files: BTreeMap::new(),
            walk_incomplete: false,
            collection: Existing::default(),
            by_path: HashMap::new(),
            reported_conflicts: HashSet::new(),
            reported_notes: HashSet::new(),
        };
        let delta = workspace.resync()?;
        Ok((workspace, delta))
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Whether `path` (project-relative) could be a controller source.
    pub fn is_relevant(&self, path: &Path) -> bool {
        !path.starts_with(&self.bruno_prefix)
            && path
                .strip_prefix(&self.source_prefix)
                .is_ok_and(|relative| self.matcher.is_relevant(relative))
    }

    /// Whether a directory (project-relative) is entered by discovery.
    pub fn is_watchable_dir(&self, path: &Path) -> bool {
        !path.starts_with(&self.bruno_prefix)
            && path
                .strip_prefix(&self.source_prefix)
                .is_ok_and(|relative| !self.matcher.is_pruned_dir(relative))
    }

    /// The collection directory relative to the *source* directory, if it lies
    /// inside it.
    pub fn collection_inside_source(&self) -> Option<PathBuf> {
        self.bruno_prefix
            .strip_prefix(&self.source_prefix)
            .ok()
            .map(Path::to_path_buf)
    }

    /// Everything from disk again: discovery, analysis, the collection, sync.
    /// The safe fallback whenever incremental correctness cannot be argued.
    pub fn resync(&mut self) -> Result<Delta, Error> {
        let discovery = discover_controllers(&self.project)?;
        self.walk_incomplete = discovery.is_incomplete();
        let mut diagnostics = discovery.diagnostics;

        self.files.clear();
        let results = analyze_each(&self.project, &discovery.files)?;
        let mut affected = HashSet::new();
        for (path, analysis) in results {
            diagnostics.extend(analysis.diagnostics.iter().cloned());
            let state = new_state(&analysis);
            affected.extend(changed_anchors(None, Some(&state)));
            self.files.insert(path, state);
        }

        let loaded = existing::load(self.project.root(), &self.project.config().bruno)?;
        diagnostics.extend(loaded.diagnostics.iter().cloned());
        self.collection = Existing {
            files: loaded.files,
            diagnostics: loaded.diagnostics,
        };
        self.rebuild_index();
        self.reported_conflicts.clear();
        self.reported_notes.clear();

        let mut delta = self.synchronize()?;
        delta.changed_files = self.files.keys().cloned().collect();
        delta.routes_affected = affected.len();
        delta.diagnostics.splice(0..0, diagnostics);
        delta.full = true;
        Ok(delta)
    }

    /// Re-examine `dirty` (project-relative paths) and synchronize the effect.
    pub fn refresh(&mut self, dirty: &BTreeSet<PathBuf>) -> Result<Delta, Error> {
        let mut affected: HashSet<u64> = HashSet::new();
        let mut changed: BTreeSet<PathBuf> = BTreeSet::new();
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut to_analyze: BTreeSet<PathBuf> = BTreeSet::new();

        for path in dirty {
            let Ok(relative) = path.strip_prefix(&self.source_prefix) else {
                continue; // not below the source directory
            };
            if path.starts_with(&self.bruno_prefix) {
                continue; // the collection is output, never input
            }
            match fs::symlink_metadata(self.project.root().join(path)) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // A file or a whole directory is gone.
                    let gone: Vec<PathBuf> = self
                        .files
                        .range(path.clone()..)
                        .take_while(|(key, _)| key.starts_with(path))
                        .map(|(key, _)| key.clone())
                        .collect();
                    for key in gone {
                        if let Some(state) = self.files.remove(&key) {
                            affected.extend(changed_anchors(Some(&state), None));
                        }
                        changed.insert(key);
                    }
                }
                Err(err) => {
                    // Cannot tell what is there: do not guess.
                    diagnostics.push(Diagnostic::new(
                        Severity::Error,
                        Some(path.clone()),
                        format!("cannot inspect: {err}"),
                    ));
                    self.walk_incomplete = true;
                }
                Ok(meta) if meta.is_file() => {
                    if self.matcher.is_relevant(relative) {
                        to_analyze.insert(path.clone());
                    }
                }
                Ok(meta) if meta.is_dir() => {
                    // A directory appeared (moved in, checked out): scan it.
                    if !self.matcher.is_pruned_dir(relative) {
                        let found = discover_in(&self.project, &self.matcher, relative)?;
                        diagnostics.extend(found.diagnostics);
                        to_analyze.extend(found.files.into_iter().map(|f| f.path));
                    }
                }
                Ok(_) => {
                    // A symlink or special file: never followed.
                    if let Some(state) = self.files.remove(path) {
                        affected.extend(changed_anchors(Some(&state), None));
                        changed.insert(path.clone());
                    }
                }
            }
        }

        let list: Vec<ControllerFile> = to_analyze
            .iter()
            .map(|path| ControllerFile { path: path.clone() })
            .collect();
        for (path, analysis) in analyze_each(&self.project, &list)? {
            diagnostics.extend(analysis.diagnostics.iter().cloned());
            let failed = analysis
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error);
            let old = self.files.remove(&path);
            let state = match old {
                // Keep the last good routes while the file is broken (an editor
                // mid-save, a typo): nothing is destroyed on a partial view.
                Some(mut old) if failed => {
                    old.failed = true;
                    diagnostics.push(Diagnostic::new(
                        Severity::Warning,
                        Some(path.clone()),
                        "keeping the previously known routes of this file until it can be analyzed again",
                    ));
                    old
                }
                old => {
                    let state = new_state(&analysis);
                    affected.extend(changed_anchors(old.as_ref(), Some(&state)));
                    state
                }
            };
            self.files.insert(path.clone(), state);
            changed.insert(path);
        }

        // Nothing relevant happened (an unrelated file, a path that vanished
        // before we looked): the state is unchanged, so is the plan.
        if changed.is_empty() && diagnostics.is_empty() {
            return Ok(Delta::default());
        }

        let mut delta = self.synchronize()?;
        delta.changed_files = changed.into_iter().collect();
        delta.routes_affected = affected.len();
        delta.diagnostics.splice(0..0, diagnostics);
        Ok(delta)
    }

    /// The desired requests for the current state (for tests and diagnostics).
    pub fn desired_requests(&mut self) -> Vec<GeneratedRequest> {
        self.desired().requests
    }

    // ---------------------------------------------------------------- internals

    /// Plan against the in-memory collection and apply the delta. Retries once
    /// if a file turned out to have changed on disk.
    fn synchronize(&mut self) -> Result<Delta, Error> {
        let mut delta = Delta::default();
        let mut last_plan = Plan::default();

        for attempt in 0..2 {
            let desired = self.desired();
            let complete = !self.walk_incomplete
                && self.files.values().all(|f| !f.failed)
                && !self.collection.is_incomplete();
            let plan = reconcile::plan(
                &Desired {
                    requests: &desired.requests,
                    skipped: &desired.skipped,
                    complete,
                },
                &self.collection,
            );
            delta.diagnostics.extend(
                desired
                    .diagnostics
                    .into_iter()
                    .filter(|d| attempt == 0 && self.note_once(d)),
            );
            last_plan = plan;
            if last_plan.blocked {
                break;
            }

            let operations = Plan {
                entries: last_plan
                    .entries
                    .iter()
                    .filter(|e| {
                        matches!(e.action, Action::Create | Action::Update | Action::Delete)
                    })
                    .cloned()
                    .collect(),
                ..Plan::default()
            };
            if operations.entries.is_empty() {
                break;
            }
            let mut result = SyncResult::from_plan(&operations);
            apply::apply(
                self.project.root(),
                &self.project.config().bruno,
                &operations,
                &mut result,
            )?;
            self.record(&operations, &result);

            delta.result.applied = true;
            delta.result.created.extend(result.created);
            delta.result.updated.extend(result.updated);
            delta.result.deleted.extend(result.deleted);
            let failed: Vec<PathBuf> = result.failed.iter().map(|(p, _)| p.clone()).collect();
            if failed.is_empty() {
                break;
            }
            if attempt == 1 {
                delta.result.failed.extend(result.failed);
                break;
            }
            // Something on disk differs from our index: re-read it and plan again.
            for path in &failed {
                self.reload(path)?;
            }
        }

        // Plan-level diagnostics and conflicts, each reported when it first appears.
        for diagnostic in last_plan.diagnostics {
            if self.note_once(&diagnostic) {
                delta.diagnostics.push(diagnostic);
            }
        }
        let mut current = HashSet::new();
        for entry in &last_plan.entries {
            if let Some(reason) = &entry.conflict {
                let key = (entry.path.clone(), reason.description());
                if !self.reported_conflicts.contains(&key) {
                    delta.result.conflicts.push(crate::sync::Conflict {
                        path: entry.path.clone(),
                        route: entry.route,
                        reason: reason.clone(),
                    });
                }
                current.insert(key);
            }
        }
        self.reported_conflicts = current;
        delta.result.unchanged = last_plan
            .entries
            .iter()
            .filter(|e| e.action == Action::Unchanged)
            .map(|e| e.path.clone())
            .collect();
        if last_plan.blocked {
            delta.diagnostics.push(Diagnostic::new(
                Severity::Error,
                None,
                "critical conflicts found: nothing was written",
            ));
            // Reported as conflicts too (once).
        }
        Ok(delta)
    }

    /// `true` the first time a diagnostic is seen (so standing conditions are
    /// not repeated on every event).
    fn note_once(&mut self, diagnostic: &Diagnostic) -> bool {
        // Bounded: in a very long session diagnostics that keep changing (a
        // parse error mentions line numbers) must not accumulate forever. After
        // the cap an old condition may be mentioned once more, which is harmless.
        if self.reported_notes.len() >= MAX_REMEMBERED_NOTES {
            self.reported_notes.clear();
        }
        self.reported_notes.insert(diagnostic.to_string())
    }

    /// Desired requests of all files, reusing the cached generation of
    /// controllers whose source and folder did not change.
    fn desired(&mut self) -> DesiredSet {
        let config: Config = self.project.config().clone();
        let folders = {
            let controllers: Vec<&Controller> = self
                .files
                .values()
                .flat_map(|state| state.controllers.iter())
                .collect();
            folder_names(controllers.iter().copied())
        };

        let mut set = DesiredSet::default();
        let mut next = folders.into_iter();
        for state in self.files.values_mut() {
            if state.parts.len() != state.controllers.len() {
                state.parts = state.controllers.iter().map(|_| None).collect();
            }
            for (controller, part) in state.controllers.iter().zip(state.parts.iter_mut()) {
                let Some(Some(folder)) = next.next() else {
                    continue; // a controller without routes
                };
                if part.as_ref().is_none_or(|p| p.folder != folder) {
                    let mut generation = Generation::default();
                    generate_controller(&config, controller, &folder, &mut generation);
                    *part = Some(Part { folder, generation });
                }
                if let Some(part) = part {
                    set.requests
                        .extend(part.generation.requests.iter().cloned());
                    set.skipped.extend(part.generation.skipped.iter().cloned());
                    set.diagnostics
                        .extend(part.generation.diagnostics.iter().cloned());
                }
            }
        }
        set
    }

    fn rebuild_index(&mut self) {
        self.by_path = self
            .collection
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (key(&f.path), i))
            .collect();
    }

    /// Update the in-memory collection from what was actually done.
    fn record(&mut self, operations: &Plan, result: &SyncResult) {
        let content: HashMap<&Path, &PlanEntry> = operations
            .entries
            .iter()
            .map(|e| (e.path.as_path(), e))
            .collect();
        for path in result.created.iter().chain(&result.updated) {
            if let Some(text) = content
                .get(path.as_path())
                .and_then(|e| e.content.as_deref())
            {
                self.upsert(ExistingFile {
                    path: path.clone(),
                    kind: classify(text),
                    full_hash: content_hash(text),
                    text: Some(text.to_owned()),
                });
            }
        }
        for path in &result.deleted {
            self.remove(path);
        }
    }

    fn upsert(&mut self, file: ExistingFile) {
        match self.by_path.get(&key(&file.path)).copied() {
            Some(index) => self.collection.files[index] = file,
            None => {
                self.by_path
                    .insert(key(&file.path), self.collection.files.len());
                self.collection.files.push(file);
            }
        }
    }

    fn remove(&mut self, path: &Path) {
        if let Some(index) = self.by_path.remove(&key(path)) {
            self.collection.files.swap_remove(index);
            if let Some(moved) = self.collection.files.get(index) {
                self.by_path.insert(key(&moved.path), index);
            }
        }
    }

    /// Re-read one collection file from disk into the index.
    fn reload(&mut self, path: &Path) -> Result<(), Error> {
        let (file, diagnostics) =
            existing::reload(self.project.root(), &self.project.config().bruno, path)?;
        // Replace, do not accumulate: a file that was unreadable once and is
        // fine now must not disable deletions for the rest of the session.
        let shown = self.project.config().bruno.join(path);
        self.collection
            .diagnostics
            .retain(|d| d.path.as_deref() != Some(shown.as_path()));
        self.collection.diagnostics.extend(diagnostics);
        match file {
            Some(file) => self.upsert(file),
            None => self.remove(path),
        }
        Ok(())
    }
}

#[derive(Default)]
struct DesiredSet {
    requests: Vec<GeneratedRequest>,
    skipped: Vec<RouteIdentity>,
    diagnostics: Vec<Diagnostic>,
}

fn new_state(analysis: &crate::nest::extract::FileAnalysis) -> FileState {
    FileState {
        controllers: analysis.controllers.clone(),
        failed: analysis
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error),
        parts: Vec::new(),
    }
}

/// The handlers (controller + handler name) whose routes differ between the
/// old and new state of a file: routes added, removed, or changed in their
/// identity (controller, file, method, path, handler) or in their parameters.
/// A changed route counts once. Source line numbers are deliberately ignored:
/// inserting a line above a controller changes no request.
fn changed_anchors<'a>(old: Option<&'a FileState>, new: Option<&'a FileState>) -> Vec<u64> {
    type Shape<'a> = (u64, &'a [crate::route::RouteParameter]);
    let routes = |state: Option<&'a FileState>| -> HashMap<u64, Shape<'a>> {
        state
            .into_iter()
            .flat_map(|state| state.controllers.iter())
            .flat_map(|controller| {
                controller.routes.iter().map(move |route| {
                    let identity = RouteIdentity::of(controller, route);
                    (
                        identity.anchor(),
                        (identity.fingerprint(), route.parameters.as_slice()),
                    )
                })
            })
            .collect()
    };
    let (old, new) = (routes(old), routes(new));
    let mut anchors: Vec<u64> = old
        .iter()
        .filter(|(anchor, shape)| new.get(anchor) != Some(shape))
        .map(|(anchor, _)| *anchor)
        .chain(
            new.keys()
                .filter(|anchor| !old.contains_key(anchor))
                .copied(),
        )
        .collect();
    anchors.sort_unstable();
    anchors.dedup();
    anchors
}

fn clean_prefix(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// Case-insensitive, separator-normalized index key (as in the reconciler).
fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use proptest::prelude::*;

    use super::*;
    use crate::bruno::generate;
    use crate::bruno::stamp;
    use crate::nest::analyze_files;
    use crate::sync::synchronize;

    fn controller(name: &str, route: &str, handler: &str) -> String {
        format!(
            "@Controller('{name}') export class {name}Controller {{ @Get('{route}') {handler}() {{}} @Post() create{name}() {{}} }}"
        )
    }

    fn project_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            write(dir.path(), path, text);
        }
        let project = Project::open(dir.path()).unwrap();
        (dir, project)
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn dirty(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn request_files(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(base, &path, out);
                } else if path.extension().is_some_and(|e| e == "bru") {
                    out.push(
                        path.strip_prefix(base)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        walk(&root.join("bruno"), &root.join("bruno"), &mut out);
        out.sort();
        out
    }

    /// Request contents with `seq` neutralized, sorted: the collection as a set.
    fn contents(root: &Path) -> Vec<String> {
        let mut out: Vec<String> = request_files(root)
            .iter()
            .map(|name| {
                stamp::with_seq(
                    &fs::read_to_string(root.join("bruno").join(name)).unwrap(),
                    0,
                )
            })
            .collect();
        out.sort();
        out
    }

    /// The Phase 5 `sync` pipeline, step by step and independent of `Workspace`.
    fn reference_sync(project: &Project) {
        let discovery = discover_controllers(project).unwrap();
        let analysis = analyze_files(project, &discovery.files).unwrap();
        let generation = generate(project.config(), &analysis.controllers);
        let complete = !discovery.is_incomplete() && !analysis.is_incomplete();
        let desired = Desired {
            requests: &generation.requests,
            skipped: &generation.skipped,
            complete,
        };
        synchronize(project.root(), &project.config().bruno, &desired, false).unwrap();
    }

    // ------------------------------------------------------------ basics

    #[test]
    fn start_synchronizes_everything() {
        let (dir, project) = project_with(&[
            (
                "src/users/users.controller.ts",
                &controller("users", ":id", "findOne"),
            ),
            (
                "src/auth/auth.controller.ts",
                &controller("auth", "login", "login"),
            ),
        ]);
        let (_, delta) = Workspace::start(project).unwrap();
        assert!(delta.full);
        assert_eq!(delta.result.created.len(), 4);
        assert_eq!(request_files(dir.path()).len(), 4);
    }

    #[test]
    fn a_modified_file_updates_only_its_requests() {
        let (dir, project) = project_with(&[
            ("src/a/a.controller.ts", &controller("a", "x", "findA")),
            ("src/b/b.controller.ts", &controller("b", "y", "findB")),
        ]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        let b_before = fs::read(dir.path().join("bruno/b/find-b.bru")).unwrap();

        write(
            dir.path(),
            "src/a/a.controller.ts",
            &controller("a", "changed/:id", "findA"),
        );
        let delta = ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        assert!(!delta.full);
        assert_eq!(
            delta.changed_files,
            [PathBuf::from("src/a/a.controller.ts")]
        );
        assert_eq!(delta.result.updated, [PathBuf::from("a/find-a.bru")]);
        assert!(delta.result.created.is_empty() && delta.result.deleted.is_empty());
        assert_eq!(
            delta.routes_affected, 1,
            "findA's old and new route are the same request"
        );
        assert_eq!(
            fs::read(dir.path().join("bruno/b/find-b.bru")).unwrap(),
            b_before
        );
        assert!(
            fs::read_to_string(dir.path().join("bruno/a/find-a.bru"))
                .unwrap()
                .contains("changed/:id")
        );
    }

    #[test]
    fn routes_affected_counts_changed_routes_not_moved_lines() {
        let src = |body: &str| {
            format!(
                "@Controller('a') export class AController {{ @Get('x') findA({body}) {{}} @Post() createA() {{}} }}"
            )
        };
        let (dir, project) = project_with(&[("src/a/a.controller.ts", &src(""))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        // Only a query parameter of one route changes: one route affected, and
        // its request is rewritten even though its identity is the same.
        write(
            dir.path(),
            "src/a/a.controller.ts",
            &src("@Query('page') p: number"),
        );
        let delta = ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        assert_eq!((delta.routes_affected, delta.result.updated.len()), (1, 1));
        // Only whitespace and comments change (every line number shifts): nothing.
        write(
            dir.path(),
            "src/a/a.controller.ts",
            &format!("// header\n\n\n{}", src("@Query('page') p: number")),
        );
        let delta = ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        assert_eq!(
            (delta.routes_affected, delta.result.has_changes()),
            (0, false)
        );
    }

    #[test]
    fn unchanged_controllers_are_not_regenerated() {
        let (dir, project) = project_with(&[
            ("src/a/a.controller.ts", &controller("a", "x", "findA")),
            ("src/b/b.controller.ts", &controller("b", "y", "findB")),
        ]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        let pointer = |ws: &Workspace, file: &str| {
            ws.files[Path::new(file)].parts[0]
                .as_ref()
                .unwrap()
                .generation
                .requests[0]
                .content
                .as_ptr()
        };
        let a_before = pointer(&ws, "src/a/a.controller.ts");
        let b_before = pointer(&ws, "src/b/b.controller.ts");
        write(
            dir.path(),
            "src/a/a.controller.ts",
            &controller("a", "other", "findA"),
        );
        ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        assert_ne!(
            pointer(&ws, "src/a/a.controller.ts"),
            a_before,
            "the changed controller is regenerated"
        );
        assert_eq!(
            pointer(&ws, "src/b/b.controller.ts"),
            b_before,
            "the other one is reused from the cache"
        );
    }

    #[test]
    fn a_new_file_creates_only_its_requests() {
        let (dir, project) =
            project_with(&[("src/a/a.controller.ts", &controller("a", "x", "findA"))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        write(
            dir.path(),
            "src/b/b.controller.ts",
            &controller("b", "y", "findB"),
        );
        let delta = ws.refresh(&dirty(&["src/b/b.controller.ts"])).unwrap();
        assert_eq!(delta.result.created.len(), 2);
        assert!(delta.result.updated.is_empty() && delta.result.deleted.is_empty());
        assert_eq!(delta.routes_affected, 2);
    }

    #[test]
    fn a_removed_file_deletes_its_requests() {
        let (dir, project) = project_with(&[
            ("src/a/a.controller.ts", &controller("a", "x", "findA")),
            ("src/b/b.controller.ts", &controller("b", "y", "findB")),
        ]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        fs::remove_file(dir.path().join("src/b/b.controller.ts")).unwrap();
        let delta = ws.refresh(&dirty(&["src/b/b.controller.ts"])).unwrap();
        assert_eq!(delta.result.deleted.len(), 2);
        assert!(!dir.path().join("bruno/b").exists());
        assert!(dir.path().join("bruno/a/find-a.bru").is_file());
    }

    #[test]
    fn a_removed_directory_removes_every_file_below_it() {
        let (dir, project) = project_with(&[
            ("src/mods/a/a.controller.ts", &controller("a", "x", "findA")),
            ("src/mods/b/b.controller.ts", &controller("b", "y", "findB")),
            ("src/keep/k.controller.ts", &controller("k", "z", "findK")),
        ]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        fs::remove_dir_all(dir.path().join("src/mods")).unwrap();
        let delta = ws.refresh(&dirty(&["src/mods"])).unwrap();
        assert_eq!(delta.changed_files.len(), 2);
        assert_eq!(delta.result.deleted.len(), 4);
        assert!(dir.path().join("bruno/k/find-k.bru").is_file());
    }

    #[test]
    fn a_directory_appearing_at_once_is_scanned() {
        let (dir, project) =
            project_with(&[("src/keep/k.controller.ts", &controller("k", "z", "findK"))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        write(
            dir.path(),
            "src/new/deep/n.controller.ts",
            &controller("n", "q", "findN"),
        );
        write(
            dir.path(),
            "src/new/deep/other.service.ts",
            "export class S {}",
        );
        write(
            dir.path(),
            "src/new/node_modules/x/x.controller.ts",
            &controller("x", "q", "findX"),
        );
        let delta = ws.refresh(&dirty(&["src/new"])).unwrap();
        assert_eq!(delta.result.created.len(), 2);
        assert!(
            !request_files(dir.path())
                .iter()
                .any(|f| f.starts_with("x/"))
        );
    }

    #[test]
    fn a_rename_updates_in_place_without_duplicates() {
        let (dir, project) =
            project_with(&[("src/a/a.controller.ts", &controller("a", "x", "findA"))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        let before = request_files(dir.path());
        fs::create_dir_all(dir.path().join("src/moved")).unwrap();
        fs::rename(
            dir.path().join("src/a/a.controller.ts"),
            dir.path().join("src/moved/a.controller.ts"),
        )
        .unwrap();
        let delta = ws
            .refresh(&dirty(&[
                "src/a/a.controller.ts",
                "src/moved/a.controller.ts",
            ]))
            .unwrap();
        assert_eq!(
            request_files(dir.path()),
            before,
            "no duplicate and no leftover"
        );
        assert_eq!(delta.result.updated.len(), 2);
        assert!(delta.result.created.is_empty() && delta.result.deleted.is_empty());
    }

    #[test]
    fn irrelevant_and_foreign_paths_do_nothing() {
        let (dir, project) =
            project_with(&[("src/a/a.controller.ts", &controller("a", "x", "findA"))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        write(dir.path(), "src/a/a.service.ts", "export class S {}");
        write(dir.path(), "src/a/data.json", "{}");
        write(dir.path(), "README.md", "x");
        write(
            dir.path(),
            "src/dist/d.controller.ts",
            &controller("d", "q", "findD"),
        );
        let delta = ws
            .refresh(&dirty(&[
                "src/a/a.service.ts",
                "src/a/data.json",
                "README.md",
                "src/dist/d.controller.ts",
                "bruno/a/find-a.bru",
                "src/missing.txt",
            ]))
            .unwrap();
        assert!(delta.changed_files.is_empty());
        assert!(!delta.result.has_changes());
        assert_eq!(delta.routes_affected, 0);
    }

    // ------------------------------------------------------------ errors

    #[test]
    fn a_file_that_stops_parsing_keeps_its_requests_and_nothing_is_deleted() {
        let (dir, project) = project_with(&[
            ("src/a/a.controller.ts", &controller("a", "x", "findA")),
            ("src/b/b.controller.ts", &controller("b", "y", "findB")),
        ]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        let before = request_files(dir.path());

        write(
            dir.path(),
            "src/a/a.controller.ts",
            "@Controller('a') export class AController { @Get( }",
        );
        let delta = ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        assert!(
            delta
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
        );
        assert!(
            delta
                .diagnostics
                .iter()
                .any(|d| d.message.contains("keeping the previously known routes"))
        );
        assert!(!delta.result.has_changes());
        assert_eq!(request_files(dir.path()), before);

        // Another file changes while the first is still broken: no deletions.
        write(
            dir.path(),
            "src/b/b.controller.ts",
            "@Controller('b') export class BController { @Get('y') findB() {} }",
        );
        let delta = ws.refresh(&dirty(&["src/b/b.controller.ts"])).unwrap();
        assert!(
            delta.result.deleted.is_empty(),
            "no destructive sync on a partial view"
        );
        assert!(
            request_files(dir.path())
                .iter()
                .any(|f| f == "b/createb.bru")
        );

        // Fixed: synchronization resumes, and the deferred deletion happens.
        write(
            dir.path(),
            "src/a/a.controller.ts",
            &controller("a", "x", "findA"),
        );
        let delta = ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        assert_eq!(delta.result.deleted, [PathBuf::from("b/createb.bru")]);
    }

    #[test]
    fn a_new_file_that_never_parsed_contributes_nothing() {
        let (dir, project) =
            project_with(&[("src/a/a.controller.ts", &controller("a", "x", "findA"))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        write(dir.path(), "src/b/b.controller.ts", "@Controller( {");
        let delta = ws.refresh(&dirty(&["src/b/b.controller.ts"])).unwrap();
        assert!(!delta.result.has_changes());
        assert!(
            delta
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
        );
    }

    // ------------------------------------------------------------ the collection changed under us

    #[test]
    fn a_request_edited_in_bruno_meanwhile_is_not_overwritten() {
        let (dir, project) =
            project_with(&[("src/a/a.controller.ts", &controller("a", "x", "findA"))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        // The user edits a generated request while watch mode is running: the
        // in-memory index does not know.
        let path = dir.path().join("bruno/a/find-a.bru");
        let edited = fs::read_to_string(&path)
            .unwrap()
            .replace("auth: inherit", "auth: bearer");
        fs::write(&path, &edited).unwrap();

        write(
            dir.path(),
            "src/a/a.controller.ts",
            &controller("a", "changed", "findA"),
        );
        let delta = ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            edited,
            "the edit survives"
        );
        assert_eq!(delta.result.conflicts.len(), 1, "{:?}", delta.result);
        assert!(
            delta
                .result
                .updated
                .iter()
                .all(|p| p != Path::new("a/find-a.bru"))
        );

        // The conflict is reported once, not on every later event.
        write(
            dir.path(),
            "src/a/a.controller.ts",
            &controller("a", "changed-again", "findA"),
        );
        let again = ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        assert!(again.result.conflicts.is_empty());
    }

    #[test]
    fn a_file_created_in_the_collection_meanwhile_is_never_clobbered() {
        let (dir, project) =
            project_with(&[("src/a/a.controller.ts", &controller("a", "x", "findA"))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        write(dir.path(), "bruno/b/find-b.bru", "hand written\n");
        write(
            dir.path(),
            "src/b/b.controller.ts",
            &controller("b", "y", "findB"),
        );
        let delta = ws.refresh(&dirty(&["src/b/b.controller.ts"])).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("bruno/b/find-b.bru")).unwrap(),
            "hand written\n"
        );
        assert_eq!(delta.result.conflicts.len(), 1, "{:?}", delta.result);
    }

    #[test]
    fn a_standing_failure_to_read_a_file_does_not_disable_deletion_forever() {
        // reload() replaces diagnostics for the path instead of accumulating.
        let (dir, project) =
            project_with(&[("src/a/a.controller.ts", &controller("a", "x", "findA"))]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        let shown = ws.project().config().bruno.join("a/find-a.bru");
        ws.collection.diagnostics.push(Diagnostic::new(
            Severity::Error,
            Some(shown),
            "cannot read file",
        ));
        assert!(ws.collection.is_incomplete());
        ws.reload(Path::new("a/find-a.bru")).unwrap();
        assert!(!ws.collection.is_incomplete());
        let _ = dir;
    }

    #[test]
    fn a_collection_inside_the_source_tree_is_never_relevant_or_watchable() {
        let (dir, _) =
            project_with(&[("users/users.controller.ts", &controller("users", "x", "f"))]);
        write(dir.path(), "nestbruno.toml", "source = \".\"\n");
        let project = Project::open(dir.path()).unwrap();
        let (ws, _) = Workspace::start(project).unwrap();
        assert!(ws.is_relevant(Path::new("users/users.controller.ts")));
        assert!(!ws.is_relevant(Path::new("bruno/users/x.controller.ts")));
        assert!(ws.is_watchable_dir(Path::new("users")));
        assert!(!ws.is_watchable_dir(Path::new("bruno")));
        assert!(!ws.is_watchable_dir(Path::new("bruno/users")));
        assert_eq!(ws.collection_inside_source(), Some(PathBuf::from("bruno")));
    }

    // ------------------------------------------------------------ long sessions

    #[test]
    fn memory_stays_bounded_over_thousands_of_changes() {
        let (dir, project) = project_with(&[
            ("src/a/a.controller.ts", &controller("a", "x", "findA")),
            ("src/b/b.controller.ts", &controller("b", "y", "findB")),
        ]);
        let (mut ws, _) = Workspace::start(project).unwrap();
        for i in 0..1500 {
            // Alternate valid edits and broken files whose error text changes
            // every time (different line numbers), the worst case for notes.
            let text = if i % 3 == 0 {
                format!("{}\n// {i}", "\n".repeat(i % 40))
                    + "@Controller('a') export class AController { @Get( }"
            } else {
                controller("a", &format!("r{}", i % 7), "findA")
            };
            write(dir.path(), "src/a/a.controller.ts", &text);
            ws.refresh(&dirty(&["src/a/a.controller.ts"])).unwrap();
        }
        assert_eq!(ws.files.len(), 2);
        assert_eq!(
            ws.collection.files.len(),
            4,
            "four requests tracked, no growth"
        );
        assert!(ws.reported_notes.len() <= MAX_REMEMBERED_NOTES);
        assert!(ws.reported_conflicts.len() <= 4);
        assert!(ws.collection.diagnostics.len() <= 4);
        assert_eq!(request_files(dir.path()).len(), 4);
    }

    // ------------------------------------------------------------ relevance

    #[test]
    fn relevance_follows_the_configuration() {
        let (_dir, project) = project_with(&[("src/a.controller.ts", &controller("a", "x", "f"))]);
        let (ws, _) = Workspace::start(project).unwrap();
        for ok in ["src/a.controller.ts", "src/deep/er/b.controller.ts"] {
            assert!(ws.is_relevant(Path::new(ok)), "{ok}");
        }
        for no in [
            "src/a.service.ts",
            "src/dist/a.controller.ts",
            "src/node_modules/x/a.controller.ts",
            "other/a.controller.ts",
            "bruno/x.bru",
            "src/a.controller.d.ts",
            ".git/x.controller.ts",
        ] {
            assert!(!ws.is_relevant(Path::new(no)), "{no}");
        }
        assert!(ws.is_watchable_dir(Path::new("src/users")));
        assert!(!ws.is_watchable_dir(Path::new("src/node_modules")));
        assert!(!ws.is_watchable_dir(Path::new("src/dist/x")));
        assert!(!ws.is_watchable_dir(Path::new("elsewhere")));
    }

    // ------------------------------------------------------------ incremental == full

    /// Source variants for a slot: valid controllers with different handlers,
    /// paths and parameters, a non-controller, and syntactically broken text.
    fn variant(slot: usize, variant: usize) -> String {
        let name = format!("c{slot}");
        match variant {
            0 => controller(&name, "x", "alpha"),
            1 => controller(&name, ":id", "alpha"),
            2 => controller(&name, ":id", "beta"),
            3 => format!(
                "@Controller('{name}') export class C{slot}Controller {{ @Get() alpha(@Query('q') q: string) {{}} @Delete(':id') gamma() {{}} }}"
            ),
            4 => "export class Plain {}".to_owned(),
            _ => format!("@Controller('{name}') export class C{slot}Controller {{ @Get( }}"),
        }
    }

    #[derive(Debug, Clone)]
    enum Op {
        Write {
            slot: usize,
            dir: usize,
            variant: usize,
        },
        Delete {
            slot: usize,
            dir: usize,
        },
        Rename {
            slot: usize,
            from: usize,
            to: usize,
        },
        Touch {
            slot: usize,
            dir: usize,
        },
    }

    fn path_of(slot: usize, dir: usize) -> String {
        format!("src/d{dir}/c{slot}.controller.ts")
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..3usize, 0..2usize, 0..6usize).prop_map(|(slot, dir, variant)| Op::Write {
                slot,
                dir,
                variant
            }),
            (0..3usize, 0..2usize).prop_map(|(slot, dir)| Op::Delete { slot, dir }),
            (0..3usize, 0..2usize, 0..2usize).prop_map(|(slot, from, to)| Op::Rename {
                slot,
                from,
                to
            }),
            (0..3usize, 0..2usize).prop_map(|(slot, dir)| Op::Touch { slot, dir }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]

        #[test]
        fn a_history_of_file_changes_gives_the_same_collection_as_a_fresh_sync(
            batches in proptest::collection::vec(proptest::collection::vec(op(), 1..4), 1..6)
        ) {
            let incremental = tempfile::tempdir().unwrap();
            write(incremental.path(), "src/d0/c0.controller.ts", &variant(0, 0));
            let project = Project::open(incremental.path()).unwrap();
            let (mut ws, _) = Workspace::start(project).unwrap();

            for batch in &batches {
                let mut touched = BTreeSet::new();
                for op in batch {
                    match op {
                        Op::Write { slot, dir, variant: v } => {
                            write(incremental.path(), &path_of(*slot, *dir), &variant(*slot, *v));
                            touched.insert(PathBuf::from(path_of(*slot, *dir)));
                        }
                        Op::Delete { slot, dir } => {
                            let _ = fs::remove_file(incremental.path().join(path_of(*slot, *dir)));
                            touched.insert(PathBuf::from(path_of(*slot, *dir)));
                        }
                        Op::Rename { slot, from, to } => {
                            let (a, b) = (incremental.path().join(path_of(*slot, *from)), incremental.path().join(path_of(*slot, *to)));
                            if a.is_file() && !b.exists() && from != to {
                                fs::create_dir_all(b.parent().unwrap()).unwrap();
                                fs::rename(&a, &b).unwrap();
                                touched.insert(PathBuf::from(path_of(*slot, *from)));
                                touched.insert(PathBuf::from(path_of(*slot, *to)));
                            }
                        }
                        Op::Touch { slot, dir } => {
                            touched.insert(PathBuf::from(path_of(*slot, *dir)));
                        }
                    }
                }
                ws.refresh(&touched).unwrap();
            }

            // Repair every broken file, as a user eventually does, so that both
            // sides see a complete source tree.
            let mut repaired = BTreeSet::new();
            for slot in 0..3 {
                for dir in 0..2 {
                    let path = incremental.path().join(path_of(slot, dir));
                    if fs::read_to_string(&path).is_ok_and(|t| t.contains("@Get( }")) {
                        write(incremental.path(), &path_of(slot, dir), &variant(slot, 4));
                        repaired.insert(PathBuf::from(path_of(slot, dir)));
                    }
                }
            }
            ws.refresh(&repaired).unwrap();

            // Reference: copy the final sources to a clean project and run the
            // Phase 5 pipeline.
            let fresh = tempfile::tempdir().unwrap();
            for slot in 0..3 {
                for dir in 0..2 {
                    if let Ok(text) = fs::read_to_string(incremental.path().join(path_of(slot, dir))) {
                        write(fresh.path(), &path_of(slot, dir), &text);
                    }
                }
            }
            fs::create_dir_all(fresh.path().join("src")).unwrap();
            reference_sync(&Project::open(fresh.path()).unwrap());

            prop_assert_eq!(contents(incremental.path()), contents(fresh.path()));
            // And the incremental state is a fixed point: nothing is left to do.
            let again = ws.refresh(&incremental_all_paths(incremental.path())).unwrap();
            prop_assert!(!again.result.has_changes(), "{:?}", again.result);
        }
    }

    fn incremental_all_paths(root: &Path) -> BTreeSet<PathBuf> {
        let mut out = BTreeSet::new();
        for slot in 0..3 {
            for dir in 0..2 {
                if root.join(path_of(slot, dir)).exists() {
                    out.insert(PathBuf::from(path_of(slot, dir)));
                }
            }
        }
        out
    }
}
