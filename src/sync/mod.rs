//! Synchronization of the desired Bruno requests with an existing collection.
//!
//! ```text
//! desired (GeneratedRequest, from the route IR)     existing .bru files
//!                         \                              /
//!                          └──────── reconcile ─────────┘      (pure)
//!                                       │
//!                                      Plan  ──▶ apply (filesystem) ──▶ SyncResult
//! ```
//!
//! The synchronization layer works on [`GeneratedRequest`]s and `.bru` files
//! only. It knows nothing about TypeScript or NestJS decorators, does no
//! terminal output and does not depend on Git.
//!
//! # Ownership model
//!
//! | Class | How recognized | Treatment |
//! | --- | --- | --- |
//! | generated, unmodified | valid NestBruno stamp and the content hashes to the stamp | may be updated or deleted |
//! | generated, modified | valid stamp, but the content differs from what was generated | updated/deleted **never**; conflict if the route changed |
//! | unknown | no stamp (any hand-written request, `folder.bru`, environments, ...) | preserved untouched |
//! | invalid | unreadable, binary, oversized, or a malformed stamp | preserved untouched, reported |
//!
//! See [`crate::bruno::stamp`] for the stamp and for which parts of a request
//! count as user-owned (everything except what the generator wrote; `seq` is
//! ignored when judging modification and is kept when a file is updated).
//!
//! # Safety rules
//!
//! 1. The whole plan is computed before anything is written.
//! 2. A *critical* conflict (duplicate route identity, or two files claiming
//!    the same identity) means **nothing at all** is written.
//! 3. Files are only ever replaced/deleted when they are positively generated
//!    and unmodified, and their content is re-verified right before the
//!    operation (a file changed in the meantime is left alone).
//! 4. Nothing is deleted when discovery or analysis was incomplete, because
//!    "no route found" might just mean "could not look".
//! 5. A request is only created where no file exists; an occupied path is a
//!    conflict, never an overwrite.

pub mod apply;
pub mod existing;
pub mod reconcile;

use std::path::{Path, PathBuf};

use crate::bruno::GeneratedRequest;
use crate::bruno::identity::RouteIdentity;
use crate::diag::{Diagnostic, Severity};
use crate::error::Error;

/// What synchronization intends to do (or did) with one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Create,
    Update,
    Unchanged,
    Delete,
    /// Not ours (or not understandable): left alone.
    Preserve,
    Conflict,
}

/// Why a file could not be synchronized automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictReason {
    /// The route changed, but the generated file has user modifications.
    GeneratedFileModified,
    /// The route no longer exists, but the generated file has user modifications.
    StaleGeneratedFileModified,
    /// A new request would go where another file already is.
    TargetOccupied,
    /// Two routes in the source have the same identity (critical).
    DuplicateRouteIdentity,
    /// Several files claim the same route identity (critical).
    DuplicateBrunoIdentity,
    /// The NestBruno metadata line is unreadable, so ownership is unclear.
    InvalidMetadata(String),
}

impl ConflictReason {
    /// Critical conflicts make synchronization write nothing at all.
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            Self::DuplicateRouteIdentity | Self::DuplicateBrunoIdentity
        )
    }

    pub fn description(&self) -> String {
        match self {
            Self::GeneratedFileModified => "the generated request was modified, and the route has changed since it was generated".to_owned(),
            Self::StaleGeneratedFileModified => "the route no longer exists, but the generated request was modified".to_owned(),
            Self::TargetOccupied => "a different file already exists where this request would be created".to_owned(),
            Self::DuplicateRouteIdentity => "several routes in the source have the same identity".to_owned(),
            Self::DuplicateBrunoIdentity => "several request files claim the same route".to_owned(),
            Self::InvalidMetadata(reason) => format!("the NestBruno metadata line is invalid ({reason})"),
        }
    }

    /// What the user can do about it.
    pub fn suggestion(&self) -> &'static str {
        match self {
            Self::GeneratedFileModified => {
                "keep your version and apply the route change by hand, or delete the file and run sync again to regenerate it"
            }
            Self::StaleGeneratedFileModified => {
                "delete the file if you no longer need it; it is left in place"
            }
            Self::TargetOccupied => "rename or remove the existing file, then run sync again",
            Self::DuplicateRouteIdentity => {
                "make the routes distinct (path or handler name); nothing was written"
            }
            Self::DuplicateBrunoIdentity => {
                "delete the 'NestBruno:' line from the docs of the copy (for example one made with Bruno's clone), or delete the copy; nothing was written"
            }
            Self::InvalidMetadata(_) => {
                "fix or delete the 'NestBruno:' line in the docs block; the file is left untouched"
            }
        }
    }
}

/// One planned operation (or observation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    /// Path relative to the collection directory.
    pub path: PathBuf,
    pub action: Action,
    /// Fingerprint of the route identity, when the entry concerns a route.
    pub route: Option<u64>,
    /// New content for `Create` and `Update`.
    pub content: Option<String>,
    /// Hash of the file as it was when planned, for `Update` and `Delete`.
    pub expected: Option<u64>,
    pub conflict: Option<ConflictReason>,
}

/// Everything synchronization would do, computed before any write.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Sorted by path (case-insensitively), one entry per file.
    pub entries: Vec<PlanEntry>,
    pub diagnostics: Vec<Diagnostic>,
    /// A critical conflict exists: nothing may be applied.
    pub blocked: bool,
}

/// A conflict as reported to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: PathBuf,
    pub route: Option<u64>,
    pub reason: ConflictReason,
}

/// The outcome of a synchronization (real or dry run). Independent of any
/// terminal formatting. Paths are relative to the collection directory.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncResult {
    /// `false` for a dry run, or when a critical conflict blocked the run.
    pub applied: bool,
    pub created: Vec<PathBuf>,
    pub updated: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub preserved: Vec<PathBuf>,
    pub conflicts: Vec<Conflict>,
    /// Operations that were planned but could not be carried out.
    pub failed: Vec<(PathBuf, String)>,
    pub diagnostics: Vec<Diagnostic>,
}

impl SyncResult {
    /// Whether the collection is fully synchronized as far as this run could tell.
    pub fn is_complete(&self) -> bool {
        self.conflicts.is_empty()
            && self.failed.is_empty()
            && !self
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
    }

    /// Whether any change was (or, in a dry run, would be) made.
    pub fn has_changes(&self) -> bool {
        !(self.created.is_empty() && self.updated.is_empty() && self.deleted.is_empty())
    }
}

/// Inputs describing the *desired* state.
pub struct Desired<'a> {
    pub requests: &'a [GeneratedRequest],
    /// Routes that exist but produced no request; their existing requests are
    /// never considered stale.
    pub skipped: &'a [RouteIdentity],
    /// Whether discovery and analysis covered everything. If not, nothing is
    /// deleted.
    pub complete: bool,
}

/// Synchronize the collection `root/bruno_dir` with `desired`.
///
/// With `dry_run` nothing is written; the result then describes what a real
/// run would do.
pub fn synchronize(
    root: &Path,
    bruno_dir: &Path,
    desired: &Desired<'_>,
    dry_run: bool,
) -> Result<SyncResult, Error> {
    let existing = existing::load(root, bruno_dir)?;
    let plan = reconcile::plan(desired, &existing);

    let mut result = SyncResult::from_plan(&plan);
    result.diagnostics.extend(existing.diagnostics);

    if plan.blocked {
        result.diagnostics.push(Diagnostic::new(
            Severity::Error,
            None,
            "critical conflicts found: nothing was written",
        ));
        return Ok(result);
    }
    if !dry_run {
        apply::apply(root, bruno_dir, &plan, &mut result)?;
    }
    Ok(result)
}

impl SyncResult {
    /// Report a plan as a result; nothing has been applied yet.
    pub fn from_plan(plan: &Plan) -> Self {
        let mut result = Self {
            diagnostics: plan.diagnostics.clone(),
            ..Self::default()
        };
        for entry in &plan.entries {
            let path = entry.path.clone();
            match entry.action {
                Action::Create => result.created.push(path),
                Action::Update => result.updated.push(path),
                Action::Unchanged => result.unchanged.push(path),
                Action::Delete => result.deleted.push(path),
                Action::Preserve => result.preserved.push(path),
                Action::Conflict => {
                    if let Some(reason) = &entry.conflict {
                        result.conflicts.push(Conflict {
                            path,
                            route: entry.route,
                            reason: reason.clone(),
                        });
                    }
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::bruno::{Generation, generate};
    use crate::config::Config;
    use crate::route::{
        Controller, HttpMethod, ParameterKind, ParameterName, Route, RouteParameter, RoutePath,
        SourceLocation,
    };

    fn route(method: HttpMethod, handler: &str, path: &str) -> Route {
        Route {
            method_name: handler.into(),
            http_method: method,
            path: RoutePath::Static(path.into()),
            parameters: vec![],
            body: crate::route::BodyShape::None,
            source: SourceLocation {
                file: Arc::from(Path::new("src/u.ts")),
                line: 1,
                column: 1,
            },
        }
    }

    fn users(routes: Vec<Route>) -> Generation {
        generate(
            &Config::default(),
            &[Controller {
                name: "UsersController".into(),
                path: RoutePath::Static("/".into()),
                source: SourceLocation {
                    file: Arc::from(Path::new("src/u.ts")),
                    line: 1,
                    column: 1,
                },
                routes,
            }],
        )
    }

    fn sync(root: &Path, g: &Generation, dry_run: bool) -> SyncResult {
        let root = root.canonicalize().unwrap();
        let desired = Desired {
            requests: &g.requests,
            skipped: &g.skipped,
            complete: true,
        };
        synchronize(&root, Path::new("bruno"), &desired, dry_run).unwrap()
    }

    fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
        if !root.exists() {
            return Vec::new();
        }
        fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    out.push((
                        format!("{}/", path.strip_prefix(base).unwrap().display()),
                        vec![],
                    ));
                    walk(base, &path, out);
                } else {
                    out.push((
                        path.strip_prefix(base).unwrap().display().to_string(),
                        fs::read(&path).unwrap(),
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    fn list() -> Route {
        route(HttpMethod::Get, "findAll", "/users")
    }
    fn one() -> Route {
        route(HttpMethod::Get, "findOne", "/users/:id")
    }
    fn with_query(mut r: Route, name: &str) -> Route {
        r.parameters.push(RouteParameter {
            kind: ParameterKind::Query,
            name: ParameterName::Named(name.into()),
            index: 0,
            type_name: None,
            fields: Vec::new(),
        });
        r
    }

    #[test]
    fn first_sync_creates_everything_and_the_second_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let g = users(vec![list(), one()]);
        let first = sync(dir.path(), &g, false);
        assert!(first.applied && first.is_complete());
        assert_eq!(first.created.len(), 2);
        assert!(dir.path().join("bruno/bruno.json").is_file());

        let before = snapshot(dir.path());
        let second = sync(dir.path(), &g, false);
        assert_eq!(
            (
                second.created.len(),
                second.updated.len(),
                second.deleted.len()
            ),
            (0, 0, 0)
        );
        assert_eq!(second.unchanged.len(), 2);
        assert!(!second.has_changes() && second.is_complete());
        assert_eq!(snapshot(dir.path()), before);
    }

    #[test]
    fn dry_run_reports_the_same_plan_but_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let g = users(vec![list(), one()]);
        let dry = sync(dir.path(), &g, true);
        assert!(!dry.applied);
        assert_eq!(dry.created.len(), 2);
        assert!(
            !dir.path().join("bruno").exists(),
            "a dry run must not even create the directory"
        );

        let real = sync(dir.path(), &g, false);
        assert_eq!(
            real.created, dry.created,
            "the dry run predicted the real run"
        );

        // And for updates and deletes.
        let changed = users(vec![with_query(list(), "q")]);
        let before = snapshot(dir.path());
        let dry = sync(dir.path(), &changed, true);
        assert_eq!((dry.updated.len(), dry.deleted.len()), (1, 1));
        assert_eq!(snapshot(dir.path()), before);
        let real = sync(dir.path(), &changed, false);
        assert_eq!(
            (real.updated.clone(), real.deleted.clone()),
            (dry.updated, dry.deleted)
        );
    }

    #[test]
    fn updates_deletes_and_empty_folders() {
        let dir = tempfile::tempdir().unwrap();
        sync(dir.path(), &users(vec![list(), one()]), false);
        let r = sync(dir.path(), &users(vec![with_query(list(), "page")]), false);
        assert_eq!(r.updated, [PathBuf::from("Users/find-all.bru")]);
        assert_eq!(r.deleted, [PathBuf::from("Users/find-one.bru")]);
        assert!(
            fs::read_to_string(dir.path().join("bruno/Users/find-all.bru"))
                .unwrap()
                .contains("~page: ")
        );
        assert!(!dir.path().join("bruno/Users/find-one.bru").exists());

        // Removing the last route removes the (now empty) folder, not bruno.json.
        let r = sync(dir.path(), &users(vec![]), false);
        assert_eq!(r.deleted.len(), 1);
        assert!(!dir.path().join("bruno/Users").exists());
        assert!(dir.path().join("bruno/bruno.json").is_file());
    }

    #[test]
    fn deleting_the_last_file_of_a_folder_and_creating_a_sibling_in_one_run_works() {
        // Regression (found by the property tests): the folder was removed
        // right after the delete, so the create in the same folder failed.
        let dir = tempfile::tempdir().unwrap();
        sync(
            dir.path(),
            &users(vec![route(HttpMethod::Get, "alpha", "/users")]),
            false,
        );
        let r = sync(
            dir.path(),
            &users(vec![route(HttpMethod::Get, "beta", "/users/:id/posts")]),
            false,
        );
        assert!(r.failed.is_empty(), "{:?}", r.failed);
        assert_eq!((r.created.len(), r.deleted.len()), (1, 1));
        assert!(dir.path().join("bruno/Users/beta.bru").is_file());
        assert!(!dir.path().join("bruno/Users/alpha.bru").exists());
    }

    #[test]
    fn a_folder_with_other_files_is_not_removed() {
        let dir = tempfile::tempdir().unwrap();
        sync(dir.path(), &users(vec![list()]), false);
        fs::write(
            dir.path().join("bruno/Users/folder.bru"),
            "meta {\n  name: Users\n}\n",
        )
        .unwrap();
        let r = sync(dir.path(), &users(vec![]), false);
        assert_eq!(r.deleted.len(), 1);
        assert!(dir.path().join("bruno/Users/folder.bru").is_file());
    }

    #[test]
    fn user_modified_requests_survive_and_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        sync(dir.path(), &users(vec![list(), one()]), false);
        let edited = fs::read_to_string(dir.path().join("bruno/Users/find-all.bru"))
            .unwrap()
            .replace("auth: inherit", "auth: bearer");
        fs::write(dir.path().join("bruno/Users/find-all.bru"), &edited).unwrap();
        let r = sync(dir.path(), &users(vec![with_query(list(), "q")]), false);
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(r.conflicts[0].reason, ConflictReason::GeneratedFileModified);
        assert_eq!(
            fs::read_to_string(dir.path().join("bruno/Users/find-all.bru")).unwrap(),
            edited
        );
        assert_eq!(r.deleted.len(), 1, "unrelated work still proceeds");
        assert!(!r.is_complete());
    }

    #[test]
    fn critical_conflicts_write_nothing_at_all() {
        let dir = tempfile::tempdir().unwrap();
        sync(dir.path(), &users(vec![list(), one()]), false);
        // A copy of a generated request (what Bruno's "clone" produces).
        let original = fs::read_to_string(dir.path().join("bruno/Users/find-all.bru")).unwrap();
        fs::write(dir.path().join("bruno/Users/copy.bru"), &original).unwrap();
        let before = snapshot(dir.path());

        let r = sync(
            dir.path(),
            &users(vec![
                with_query(list(), "q"),
                route(HttpMethod::Post, "create", "/users"),
            ]),
            false,
        );
        assert!(!r.applied);
        assert_eq!(
            r.conflicts
                .iter()
                .filter(|c| c.reason == ConflictReason::DuplicateBrunoIdentity)
                .count(),
            2
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("nothing was written"))
        );
        assert_eq!(snapshot(dir.path()), before);
    }

    #[test]
    fn duplicate_routes_in_the_source_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let r = sync(dir.path(), &users(vec![list(), list()]), false);
        assert!(!r.applied && !dir.path().join("bruno").exists());
        assert_eq!(r.conflicts.len(), 2);
    }

    #[test]
    fn unknown_files_are_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bruno/environments")).unwrap();
        fs::write(
            dir.path().join("bruno/environments/local.bru"),
            "vars {\n  a: 1\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("bruno/mine.bru"),
            "meta {\n  name: mine\n}\n",
        )
        .unwrap();
        let r = sync(dir.path(), &users(vec![list()]), false);
        assert_eq!(r.preserved.len(), 2);
        let r = sync(dir.path(), &users(vec![]), false);
        assert_eq!(r.preserved.len(), 2);
        assert_eq!(
            fs::read_to_string(dir.path().join("bruno/mine.bru")).unwrap(),
            "meta {\n  name: mine\n}\n"
        );
        assert!(dir.path().join("bruno/environments/local.bru").is_file());
    }

    #[test]
    fn a_new_request_never_overwrites_a_hand_written_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bruno/Users")).unwrap();
        fs::write(dir.path().join("bruno/Users/find-all.bru"), "my own\n").unwrap();
        let r = sync(dir.path(), &users(vec![list()]), false);
        assert_eq!(r.conflicts[0].reason, ConflictReason::TargetOccupied);
        assert_eq!(
            fs::read_to_string(dir.path().join("bruno/Users/find-all.bru")).unwrap(),
            "my own\n"
        );
    }

    #[test]
    fn incomplete_analysis_prevents_deletion() {
        let dir = tempfile::tempdir().unwrap();
        sync(dir.path(), &users(vec![list(), one()]), false);
        let g = users(vec![list()]);
        let desired = Desired {
            requests: &g.requests,
            skipped: &g.skipped,
            complete: false,
        };
        let root = dir.path().canonicalize().unwrap();
        let r = synchronize(&root, Path::new("bruno"), &desired, false).unwrap();
        assert!(r.deleted.is_empty());
        assert!(dir.path().join("bruno/Users/find-one.bru").is_file());
    }

    #[test]
    fn user_seq_survives_an_update() {
        let dir = tempfile::tempdir().unwrap();
        sync(dir.path(), &users(vec![list(), one()]), false);
        let path = dir.path().join("bruno/Users/find-one.bru");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, crate::bruno::stamp::with_seq(&text, 99)).unwrap();
        let g = users(vec![list(), with_query(one(), "include")]);
        let r = sync(dir.path(), &g, false);
        assert_eq!(r.updated.len(), 1);
        assert_eq!(
            crate::bruno::stamp::read_seq(&fs::read_to_string(&path).unwrap()),
            Some(99)
        );
    }

    // ---- properties

    mod properties {
        use proptest::prelude::*;

        use super::*;
        use crate::bruno::stamp;

        /// One route of a generated source state. Handlers are unique within a
        /// state (a duplicate route is a separate, tested, critical conflict).
        #[derive(Debug, Clone)]
        struct Spec {
            handler: usize,
            method: usize,
            path: usize,
            query: Option<usize>,
        }

        fn routes(state: &[Spec]) -> Vec<Route> {
            const METHODS: [HttpMethod; 4] = [
                HttpMethod::Get,
                HttpMethod::Post,
                HttpMethod::Put,
                HttpMethod::Delete,
            ];
            const PATHS: [&str; 4] = ["/users", "/users/:id", "/users/:id/posts", "/health"];
            let mut seen = std::collections::HashSet::new();
            state
                .iter()
                .filter(|spec| seen.insert(spec.handler))
                .map(|spec| {
                    let r = route(
                        METHODS[spec.method],
                        &format!("handler{}", spec.handler),
                        PATHS[spec.path],
                    );
                    match spec.query {
                        Some(q) => with_query(r, &format!("q{q}")),
                        None => r,
                    }
                })
                .collect()
        }

        fn state() -> impl Strategy<Value = Vec<Spec>> {
            proptest::collection::vec(
                (
                    0..6usize,
                    0..4usize,
                    0..4usize,
                    proptest::option::of(0..3usize),
                )
                    .prop_map(|(handler, method, path, query)| Spec {
                        handler,
                        method,
                        path,
                        query,
                    }),
                0..6,
            )
        }

        /// File contents with the user-owned `seq` neutralized, sorted: the
        /// collection as a set of requests, independent of file names and order.
        fn contents(root: &Path) -> Vec<String> {
            let mut out: Vec<String> = snapshot(&root.join("bruno"))
                .into_iter()
                .filter(|(name, _)| name.ends_with(".bru"))
                .map(|(_, bytes)| stamp::with_seq(&String::from_utf8(bytes).unwrap(), 0))
                .collect();
            out.sort();
            out
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(48))]

            #[test]
            fn applying_a_history_of_changes_equals_syncing_the_final_state_once(
                history in proptest::collection::vec(state(), 1..5)
            ) {
                let incremental = tempfile::tempdir().unwrap();
                for step in &history {
                    let g = users(routes(step));
                    let first = sync(incremental.path(), &g, false);
                    prop_assert!(first.is_complete(), "{:?} {:?}", first.conflicts, first.failed);
                    // Every step is idempotent.
                    let again = sync(incremental.path(), &g, false);
                    prop_assert!(!again.has_changes(), "second run changed {:?}", again);
                }

                let fresh = tempfile::tempdir().unwrap();
                let last = users(routes(&history[history.len() - 1]));
                sync(fresh.path(), &last, false);

                prop_assert_eq!(contents(incremental.path()), contents(fresh.path()));
            }

            #[test]
            fn a_dry_run_never_writes_and_predicts_the_real_run(
                before in state(), after in state()
            ) {
                let dir = tempfile::tempdir().unwrap();
                sync(dir.path(), &users(routes(&before)), false);
                let target = users(routes(&after));
                let snapshot_before = snapshot(dir.path());
                let dry = sync(dir.path(), &target, true);
                prop_assert_eq!(snapshot(dir.path()), snapshot_before);
                let real = sync(dir.path(), &target, false);
                prop_assert_eq!(&dry.created, &real.created);
                prop_assert_eq!(&dry.updated, &real.updated);
                prop_assert_eq!(&dry.deleted, &real.deleted);
            }

            #[test]
            fn user_modified_files_are_never_changed_by_any_history(
                history in proptest::collection::vec(state(), 1..4),
                victim in 0..6usize,
            ) {
                let dir = tempfile::tempdir().unwrap();
                sync(dir.path(), &users(routes(&history[0])), false);
                // The user edits one generated request.
                let edited = snapshot(&dir.path().join("bruno"))
                    .into_iter()
                    .filter(|(n, _)| n.ends_with(".bru"))
                    .nth(victim);
                let Some((name, bytes)) = edited else { return Ok(()); };
                let mut text = String::from_utf8(bytes).unwrap();
                text.push_str("\n# my notes\n");
                fs::write(dir.path().join("bruno").join(&name), &text).unwrap();

                for step in &history[1..] {
                    sync(dir.path(), &users(routes(step)), false);
                }
                let after = fs::read_to_string(dir.path().join("bruno").join(&name)).unwrap();
                prop_assert_eq!(after, text, "the user's edit must survive every later sync");
            }
        }
    }

    // ---- the applier against hostile or stale plans

    fn entry(
        path: &str,
        action: Action,
        content: Option<&str>,
        expected: Option<u64>,
    ) -> PlanEntry {
        PlanEntry {
            path: path.into(),
            action,
            route: None,
            content: content.map(str::to_owned),
            expected,
            conflict: None,
        }
    }

    fn apply_plan(root: &Path, entries: Vec<PlanEntry>) -> SyncResult {
        let plan = Plan {
            entries,
            ..Plan::default()
        };
        let mut result = SyncResult::from_plan(&plan);
        let root = root.canonicalize().unwrap();
        apply::apply(&root, Path::new("bruno"), &plan, &mut result).unwrap();
        result
    }

    #[test]
    fn a_plan_with_unsafe_paths_cannot_write_or_delete_outside_the_collection() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bruno")).unwrap();
        fs::write(dir.path().join("victim.bru"), "precious").unwrap();
        let victim_hash = crate::bruno::identity::content_hash("precious");
        let before = snapshot(dir.path());
        let r = apply_plan(
            dir.path(),
            vec![
                entry("../escape.bru", Action::Create, Some("x"), None),
                entry("../victim.bru", Action::Delete, None, Some(victim_hash)),
                entry(
                    "../victim.bru",
                    Action::Update,
                    Some("pwned"),
                    Some(victim_hash),
                ),
                entry("a/../../escape2.bru", Action::Create, Some("x"), None),
                entry("/etc/passwd", Action::Delete, None, Some(0)),
                entry("..", Action::Delete, None, Some(0)),
                entry("", Action::Create, Some("x"), None),
            ],
        );
        assert_eq!(r.failed.len(), 7, "{:?}", r.failed);
        assert!(r.created.is_empty() && r.updated.is_empty() && r.deleted.is_empty());
        // Only the collection's own bruno.json may have appeared; nothing else
        // changed, inside or outside the collection.
        let after: Vec<_> = snapshot(dir.path())
            .into_iter()
            .filter(|(name, _)| name != "bruno/bruno.json")
            .collect();
        assert_eq!(after, before);
        assert_eq!(
            fs::read_to_string(dir.path().join("victim.bru")).unwrap(),
            "precious"
        );
    }

    #[test]
    fn a_file_changed_after_planning_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bruno/U")).unwrap();
        let path = dir.path().join("bruno/U/a.bru");
        fs::write(&path, "version one").unwrap();
        let planned = crate::bruno::identity::content_hash("version one");
        fs::write(&path, "the user edited this just now").unwrap();

        let r = apply_plan(
            dir.path(),
            vec![entry(
                "U/a.bru",
                Action::Update,
                Some("generated"),
                Some(planned),
            )],
        );
        assert_eq!(r.failed.len(), 1);
        assert!(r.failed[0].1.contains("changed since"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "the user edited this just now"
        );

        let r = apply_plan(
            dir.path(),
            vec![entry("U/a.bru", Action::Delete, None, Some(planned))],
        );
        assert_eq!(r.failed.len(), 1);
        assert!(path.exists());
    }

    #[test]
    fn a_file_appearing_after_planning_is_never_overwritten_by_a_create() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bruno/U")).unwrap();
        fs::write(dir.path().join("bruno/U/a.bru"), "appeared meanwhile").unwrap();
        let r = apply_plan(
            dir.path(),
            vec![entry("U/a.bru", Action::Create, Some("new"), None)],
        );
        assert_eq!(r.failed.len(), 1);
        assert_eq!(
            fs::read_to_string(dir.path().join("bruno/U/a.bru")).unwrap(),
            "appeared meanwhile"
        );
    }

    #[test]
    fn failures_do_not_stop_the_remaining_operations() {
        let dir = tempfile::tempdir().unwrap();
        let r = apply_plan(
            dir.path(),
            vec![
                entry("../bad.bru", Action::Create, Some("x"), None),
                entry("Good/ok.bru", Action::Create, Some("fine\n"), None),
            ],
        );
        assert_eq!((r.created.len(), r.failed.len()), (1, 1));
        assert_eq!(
            fs::read_to_string(dir.path().join("bruno/Good/ok.bru")).unwrap(),
            "fine\n"
        );
    }

    #[cfg(unix)]
    mod unix {
        use std::os::unix::fs::symlink;

        use super::*;

        #[test]
        fn a_file_swapped_for_a_symlink_is_never_deleted_or_replaced_through() {
            let dir = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let victim = outside.path().join("victim.txt");
            fs::write(&victim, "precious").unwrap();
            fs::create_dir_all(dir.path().join("bruno/U")).unwrap();
            symlink(&victim, dir.path().join("bruno/U/a.bru")).unwrap();
            let h = crate::bruno::identity::content_hash("precious");
            let r = apply_plan(
                dir.path(),
                vec![
                    entry("U/a.bru", Action::Delete, None, Some(h)),
                    entry("U/a.bru", Action::Update, Some("pwned"), Some(h)),
                ],
            );
            assert_eq!(r.failed.len(), 2);
            assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
            assert!(
                fs::symlink_metadata(dir.path().join("bruno/U/a.bru"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }

        #[test]
        fn a_folder_swapped_for_a_symlink_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("a.bru"), "precious").unwrap();
            fs::create_dir_all(dir.path().join("bruno")).unwrap();
            symlink(outside.path(), dir.path().join("bruno/U")).unwrap();
            let h = crate::bruno::identity::content_hash("precious");
            let r = apply_plan(
                dir.path(),
                vec![
                    entry("U/a.bru", Action::Delete, None, Some(h)),
                    entry("U/new.bru", Action::Create, Some("x"), None),
                ],
            );
            assert_eq!(r.failed.len(), 2);
            assert_eq!(
                fs::read_to_string(outside.path().join("a.bru")).unwrap(),
                "precious"
            );
            assert!(!outside.path().join("new.bru").exists());
        }

        #[test]
        fn a_symlinked_collection_is_a_fatal_error_and_nothing_is_written() {
            let dir = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            symlink(outside.path(), dir.path().join("bruno")).unwrap();
            let g = users(vec![list()]);
            let root = dir.path().canonicalize().unwrap();
            let desired = Desired {
                requests: &g.requests,
                skipped: &g.skipped,
                complete: true,
            };
            let err = synchronize(&root, Path::new("bruno"), &desired, false).unwrap_err();
            assert!(matches!(err, Error::UnsafeOutput { .. }));
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        }
    }
}
