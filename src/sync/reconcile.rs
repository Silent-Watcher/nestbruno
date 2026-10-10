//! The reconciler: desired requests + existing files give a [`Plan`].
//!
//! Pure: no filesystem access, no clock, no randomness, so every rule is
//! unit-testable and the result is deterministic. Lookups use hash maps, so the
//! cost is linear in the number of requests and files.
//!
//! # Rules
//!
//! *Matching.* A desired request and an existing generated file are the same
//! request if their stamp `id` is equal; otherwise if their `key` (controller
//! name + method + path) is equal, otherwise if their `anchor` (controller
//! name + handler) is equal; the weaker two **only when unique on both sides and
//! not contradicted** (a file plausibly claimed by two different routes is
//! ambiguous and matched to neither). This follows handler renames and
//! controller moves (`key`) and path or method edits (`anchor`). A file keeps its location when it is updated.
//!
//! *Per match.* If the stamp already records the generated representation
//! (`id` and `hash` equal), nothing changed: [`Action::Unchanged`], even if the
//! user edited the file (their edits are preserved, there is nothing to
//! merge). Otherwise: unmodified file gives [`Action::Update`] (keeping its
//! `seq`); modified file gives a conflict.
//!
//! *No match, desired request.* Nothing at the target path gives
//! [`Action::Create`]. If the path holds another route's generated request
//! (an earlier in-place update kept an old name) the request is created under a
//! deterministic alternative name instead. An unstamped file that is byte-identical (ignoring
//! `seq`) to what would be generated is adopted ([`Action::Update`], adds the
//! stamp). A stale unmodified generated file at the path is replaced
//! ([`Action::Update`]). Anything else there is a `TargetOccupied` conflict.
//!
//! *No match, generated file.* Its route is gone: unmodified gives
//! [`Action::Delete`] (only if the run was complete), modified gives a
//! conflict, and a file whose route still exists but could not be generated
//! this time is preserved.
//!
//! *Other files.* Unknown and invalid files are preserved. A malformed
//! NestBruno line is a conflict.
//!
//! *Critical conflicts.* Equal identity on two desired routes, or on two files,
//! sets [`Plan::blocked`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::existing::{Existing, Kind};
use super::{Action, ConflictReason, Desired, Plan, PlanEntry};
use crate::bruno::stamp::{self, Parsed};
use crate::diag::{Diagnostic, Severity};

struct Want<'a> {
    id: u64,
    key: u64,
    anchor: u64,
    hash: u64,
    path: PathBuf,
    content: &'a str,
}

/// The weaker identities tried after an exact `id` match.
#[derive(Clone, Copy)]
enum Tier {
    /// Controller + method + path.
    Key,
    /// Controller + handler.
    Anchor,
}

impl Tier {
    fn of_want(self, want: &Want<'_>) -> u64 {
        match self {
            Self::Key => want.key,
            Self::Anchor => want.anchor,
        }
    }

    fn of_stamp(self, stamp: &stamp::Stamp) -> u64 {
        match self {
            Self::Key => stamp.key,
            Self::Anchor => stamp.anchor,
        }
    }
}

/// Compute the plan.
pub fn plan(desired: &Desired<'_>, existing: &Existing) -> Plan {
    let mut plan = Plan::default();
    let mut entries: Vec<PlanEntry> = Vec::new();
    let allow_delete = desired.complete && !existing.is_incomplete();

    // ---- desired side
    let mut wants: Vec<Want<'_>> = Vec::new();
    for request in desired.requests {
        let Parsed::Stamped(s) = stamp::parse(&request.content) else {
            plan.diagnostics.push(Diagnostic::new(
                Severity::Error,
                Some(PathBuf::from(&request.folder).join(&request.file_name)),
                "internal error: a generated request carries no valid stamp; it was skipped",
            ));
            continue;
        };
        wants.push(Want {
            id: s.id,
            key: s.key,
            anchor: s.anchor,
            hash: s.hash,
            path: PathBuf::from(&request.folder).join(&request.file_name),
            content: &request.content,
        });
    }

    let mut want_ids: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, want) in wants.iter().enumerate() {
        want_ids.entry(want.id).or_default().push(index);
    }
    let mut blocked_ids: HashSet<u64> = HashSet::new();
    for (id, indexes) in &want_ids {
        if indexes.len() > 1 {
            blocked_ids.insert(*id);
            plan.blocked = true;
            for &index in indexes {
                entries.push(conflict(
                    &wants[index].path,
                    Some(*id),
                    ConflictReason::DuplicateRouteIdentity,
                ));
            }
        }
    }

    // ---- existing side
    let mut file_ids: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, file) in existing.files.iter().enumerate() {
        if let Kind::Generated { stamp, .. } = &file.kind {
            file_ids.entry(stamp.id).or_default().push(index);
        }
    }
    for (id, indexes) in &file_ids {
        if indexes.len() > 1 {
            blocked_ids.insert(*id);
            plan.blocked = true;
            for &index in indexes {
                entries.push(conflict(
                    &existing.files[index].path,
                    Some(*id),
                    ConflictReason::DuplicateBrunoIdentity,
                ));
            }
        }
    }
    let by_path: HashMap<String, usize> = existing
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| (lower(&file.path), index))
        .collect();

    // ---- matching
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut matched_want: HashSet<usize> = HashSet::new();
    let mut matched_file: HashSet<usize> = HashSet::new();

    for (w, want) in wants.iter().enumerate() {
        if blocked_ids.contains(&want.id) {
            continue;
        }
        if let Some([f]) = file_ids.get(&want.id).map(Vec::as_slice) {
            pairs.push((w, *f));
            matched_want.insert(w);
            matched_file.insert(*f);
        }
    }

    // Second and third tier: the same route after a handler rename or move
    // (`key`), or the same handler after a path or method change (`anchor`).
    // A candidate pair needs a value that is unique on both sides. Candidates
    // of both tiers are collected first and a pair is accepted only if neither
    // its request nor its file also appears in a *different* candidate pair:
    // one file plausibly claimed by two routes is ambiguous, and guessing would
    // be wrong half the time.
    let mut candidates: Vec<(usize, usize)> = Vec::new();
    for tier in [Tier::Key, Tier::Anchor] {
        let mut want_groups: HashMap<u64, Vec<usize>> = HashMap::new();
        for (w, want) in wants.iter().enumerate() {
            if !matched_want.contains(&w) && !blocked_ids.contains(&want.id) {
                want_groups.entry(tier.of_want(want)).or_default().push(w);
            }
        }
        let mut file_groups: HashMap<u64, Vec<usize>> = HashMap::new();
        for (f, file) in existing.files.iter().enumerate() {
            if let Kind::Generated { stamp, .. } = &file.kind
                && !matched_file.contains(&f)
                && !blocked_ids.contains(&stamp.id)
            {
                file_groups.entry(tier.of_stamp(stamp)).or_default().push(f);
            }
        }
        for (value, ws) in &want_groups {
            if let ([w], Some([f])) = (ws.as_slice(), file_groups.get(value).map(Vec::as_slice)) {
                candidates.push((*w, *f));
            }
        }
    }
    candidates.sort_unstable();
    candidates.dedup(); // the same pair found by both tiers is one, stronger, candidate
    let mut per_want: HashMap<usize, usize> = HashMap::new();
    let mut per_file: HashMap<usize, usize> = HashMap::new();
    for &(w, f) in &candidates {
        *per_want.entry(w).or_default() += 1;
        *per_file.entry(f).or_default() += 1;
    }
    for (w, f) in candidates {
        if per_want[&w] == 1 && per_file[&f] == 1 {
            pairs.push((w, f));
            matched_want.insert(w);
            matched_file.insert(f);
        }
    }
    pairs.sort_unstable();

    // ---- matched pairs
    for &(w, f) in &pairs {
        let want = &wants[w];
        let file = &existing.files[f];
        let Kind::Generated { stamp, modified } = &file.kind else {
            continue;
        };
        if stamp.id == want.id && stamp.hash == want.hash {
            entries.push(entry(&file.path, Action::Unchanged, Some(want.id)));
        } else if *modified {
            entries.push(conflict(
                &file.path,
                Some(want.id),
                ConflictReason::GeneratedFileModified,
            ));
        } else {
            entries.push(update(
                &file.path,
                want,
                file.text.as_deref(),
                file.full_hash,
            ));
        }
    }

    // ---- desired requests without a match
    let skipped_ids: HashSet<u64> = desired.skipped.iter().map(|i| i.fingerprint()).collect();
    let skipped_keys: HashSet<u64> = desired.skipped.iter().map(|i| i.key()).collect();
    let skipped_anchors: HashSet<u64> = desired.skipped.iter().map(|i| i.anchor()).collect();
    let is_skipped = |stamp: &stamp::Stamp| {
        skipped_ids.contains(&stamp.id)
            || skipped_keys.contains(&stamp.key)
            || skipped_anchors.contains(&stamp.anchor)
    };

    let mut consumed_file: HashSet<usize> = HashSet::new();
    for (w, want) in wants.iter().enumerate() {
        if matched_want.contains(&w) || blocked_ids.contains(&want.id) {
            continue;
        }
        let Some(&f) = by_path.get(&lower(&want.path)) else {
            entries.push(PlanEntry {
                content: Some(want.content.to_owned()),
                ..entry(&want.path, Action::Create, Some(want.id))
            });
            continue;
        };
        let file = &existing.files[f];
        match &file.kind {
            Kind::InvalidMetadata(_) => {} // reported once, below
            Kind::Generated {
                stamp,
                modified: false,
            } if allow_delete
                && !matched_file.contains(&f)
                && !blocked_ids.contains(&stamp.id)
                && !is_skipped(stamp) =>
            {
                // A stale generated file in the way: replace it in place.
                consumed_file.insert(f);
                entries.push(update(
                    &file.path,
                    want,
                    file.text.as_deref(),
                    file.full_hash,
                ));
            }
            Kind::Unknown
                if file
                    .text
                    .as_deref()
                    .is_some_and(|text| stamp::normalized_hash(text) == want.hash) =>
            {
                // Byte-identical (apart from `seq`) to what we would generate:
                // an unstamped file from an earlier version. Adopt it.
                consumed_file.insert(f);
                entries.push(update(
                    &file.path,
                    want,
                    file.text.as_deref(),
                    file.full_hash,
                ));
            }
            Kind::Generated { .. } if matched_file.contains(&f) => {
                // The name belongs to another route's request (kept in place by
                // an earlier update). Not a conflict: use a deterministic
                // alternative name, unless that is taken too.
                let alternative = alternative_path(&want.path, want.id);
                if by_path.contains_key(&lower(&alternative)) {
                    entries.push(conflict(
                        &file.path,
                        Some(want.id),
                        ConflictReason::TargetOccupied,
                    ));
                } else {
                    entries.push(PlanEntry {
                        content: Some(want.content.to_owned()),
                        ..entry(&alternative, Action::Create, Some(want.id))
                    });
                }
            }
            _ => {
                // One conflict per file: a generated occupant must not also be
                // reported again as a stale file below.
                if matches!(file.kind, Kind::Generated { .. }) {
                    consumed_file.insert(f);
                }
                entries.push(conflict(
                    &file.path,
                    Some(want.id),
                    ConflictReason::TargetOccupied,
                ));
            }
        }
    }

    // ---- existing files without a match
    let mut deletions_skipped = false;
    for (f, file) in existing.files.iter().enumerate() {
        if matched_file.contains(&f) || consumed_file.contains(&f) {
            continue;
        }
        match &file.kind {
            Kind::Generated { stamp, modified } => {
                if blocked_ids.contains(&stamp.id) {
                    continue; // already reported as a duplicate
                }
                if is_skipped(stamp) {
                    plan.diagnostics.push(Diagnostic::new(
                        Severity::Warning,
                        Some(file.path.clone()),
                        "its route still exists but no request could be generated for it this time; the file is left in place",
                    ));
                    entries.push(entry(&file.path, Action::Preserve, Some(stamp.id)));
                } else if *modified {
                    entries.push(conflict(
                        &file.path,
                        Some(stamp.id),
                        ConflictReason::StaleGeneratedFileModified,
                    ));
                } else if allow_delete {
                    entries.push(PlanEntry {
                        expected: Some(file.full_hash),
                        ..entry(&file.path, Action::Delete, Some(stamp.id))
                    });
                } else {
                    deletions_skipped = true;
                    entries.push(entry(&file.path, Action::Preserve, Some(stamp.id)));
                }
            }
            Kind::Unknown | Kind::Invalid(_) => {
                entries.push(entry(&file.path, Action::Preserve, None))
            }
            Kind::InvalidMetadata(reason) => {
                entries.push(conflict(
                    &file.path,
                    None,
                    ConflictReason::InvalidMetadata(reason.clone()),
                ));
            }
        }
    }
    if deletions_skipped {
        plan.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            None,
            "requests of routes that no longer exist were not deleted because source discovery or analysis was incomplete",
        ));
    }

    entries.sort_by(|a, b| {
        lower(&a.path)
            .cmp(&lower(&b.path))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.action.cmp(&b.action))
    });
    plan.entries = entries;
    plan
}

/// `Folder/name.bru` becomes `Folder/name-<8 hex of the route id>.bru`.
fn alternative_path(path: &std::path::Path, id: u64) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("request");
    let name = format!("{stem}-{:08x}.bru", id & 0xffff_ffff);
    path.with_file_name(name)
}

fn lower(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn entry(path: &std::path::Path, action: Action, route: Option<u64>) -> PlanEntry {
    PlanEntry {
        path: path.to_owned(),
        action,
        route,
        content: None,
        expected: None,
        conflict: None,
    }
}

fn conflict(path: &std::path::Path, route: Option<u64>, reason: ConflictReason) -> PlanEntry {
    PlanEntry {
        conflict: Some(reason),
        ..entry(path, Action::Conflict, route)
    }
}

/// Replace the file at `path` with the desired content, keeping the user's `seq`.
fn update(
    path: &std::path::Path,
    want: &Want<'_>,
    existing_text: Option<&str>,
    full_hash: u64,
) -> PlanEntry {
    let content = match existing_text.and_then(stamp::read_seq) {
        Some(seq) => stamp::with_seq(want.content, seq),
        None => want.content.to_owned(),
    };
    PlanEntry {
        content: Some(content),
        expected: Some(full_hash),
        ..entry(path, Action::Update, Some(want.id))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::bruno::identity::{RouteIdentity, content_hash};
    use crate::bruno::{Generation, generate};
    use crate::config::Config;
    use crate::route::{
        Controller, HttpMethod, ParameterKind, ParameterName, Route, RouteParameter, RoutePath,
        SourceLocation,
    };
    use crate::sync::existing::{ExistingFile, classify};

    fn location(file: &str) -> SourceLocation {
        SourceLocation {
            file: Arc::from(Path::new(file)),
            line: 1,
            column: 1,
        }
    }

    fn route(method: HttpMethod, handler: &str, path: &str) -> Route {
        Route {
            method_name: handler.into(),
            http_method: method,
            path: RoutePath::Static(path.into()),
            parameters: vec![],
            source: location("src/users.controller.ts"),
        }
    }

    fn with_query(mut r: Route, name: &str) -> Route {
        r.parameters.push(RouteParameter {
            kind: ParameterKind::Query,
            name: ParameterName::Named(name.into()),
            index: 0,
        });
        r
    }

    fn controller(name: &str, file: &str, routes: Vec<Route>) -> Controller {
        Controller {
            name: name.into(),
            path: RoutePath::Static("/".into()),
            source: location(file),
            routes,
        }
    }

    fn users(routes: Vec<Route>) -> Generation {
        generate(
            &Config::default(),
            &[controller(
                "UsersController",
                "src/users.controller.ts",
                routes,
            )],
        )
    }

    fn list() -> Route {
        route(HttpMethod::Get, "findAll", "/users")
    }
    fn one() -> Route {
        route(HttpMethod::Get, "findOne", "/users/:id")
    }

    fn path_of(g: &Generation, index: usize) -> String {
        format!(
            "{}/{}",
            g.requests[index].folder, g.requests[index].file_name
        )
    }

    /// Existing files as the loader would produce them (text kept for all).
    fn existing(files: &[(&str, &str)]) -> Existing {
        Existing {
            files: files
                .iter()
                .map(|(path, text)| ExistingFile {
                    path: PathBuf::from(path),
                    kind: classify(text),
                    full_hash: content_hash(text),
                    text: Some((*text).to_owned()),
                })
                .collect(),
            diagnostics: vec![],
        }
    }

    fn run(g: &Generation, ex: &Existing) -> Plan {
        plan(
            &Desired {
                requests: &g.requests,
                skipped: &g.skipped,
                complete: true,
            },
            ex,
        )
    }

    fn actions(p: &Plan) -> Vec<(String, Action)> {
        p.entries
            .iter()
            .map(|e| (e.path.to_string_lossy().into_owned(), e.action))
            .collect()
    }

    fn find<'a>(p: &'a Plan, path: &str) -> &'a PlanEntry {
        p.entries
            .iter()
            .find(|e| e.path == Path::new(path))
            .unwrap_or_else(|| panic!("no entry for {path}: {:?}", actions(p)))
    }

    /// Existing files equal to what `g` generates.
    fn synced(g: &Generation) -> Vec<(String, String)> {
        (0..g.requests.len())
            .map(|i| (path_of(g, i), g.requests[i].content.clone()))
            .collect()
    }

    fn ex_of(files: &[(String, String)]) -> Existing {
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        existing(&refs)
    }

    // ------------------------------------------------------------ create / unchanged

    #[test]
    fn new_routes_are_created() {
        let g = users(vec![list(), one()]);
        let p = run(&g, &Existing::default());
        assert_eq!(
            actions(&p),
            [
                ("Users/find-all.bru".into(), Action::Create),
                ("Users/find-one.bru".into(), Action::Create)
            ]
        );
        assert_eq!(
            find(&p, "Users/find-all.bru").content.as_deref(),
            Some(g.requests[0].content.as_str())
        );
        assert!(!p.blocked);
    }

    #[test]
    fn identical_files_are_unchanged_and_carry_no_operation() {
        let g = users(vec![list(), one()]);
        let p = run(&g, &ex_of(&synced(&g)));
        assert!(
            p.entries
                .iter()
                .all(|e| e.action == Action::Unchanged && e.content.is_none())
        );
        assert_eq!(p.entries.len(), 2);
    }

    // ------------------------------------------------------------ update

    #[test]
    fn unmodified_generated_files_are_updated_in_place_when_the_route_changes() {
        let old = users(vec![list()]);
        let new = users(vec![with_query(list(), "page")]);
        let p = run(&new, &ex_of(&synced(&old)));
        let e = find(&p, "Users/find-all.bru");
        assert_eq!(e.action, Action::Update);
        let content = e.content.as_deref().unwrap();
        assert!(content.contains("~page: "));
        assert_eq!(e.expected, Some(content_hash(&old.requests[0].content)));
    }

    #[test]
    fn an_update_keeps_the_users_seq() {
        let old = users(vec![list()]);
        let reordered = stamp::with_seq(&old.requests[0].content, 7);
        let new = users(vec![with_query(list(), "page")]);
        let p = run(&new, &ex_of(&[("Users/find-all.bru".into(), reordered)]));
        let e = find(&p, "Users/find-all.bru");
        assert_eq!(
            e.action,
            Action::Update,
            "a changed seq alone is not a modification"
        );
        assert_eq!(stamp::read_seq(e.content.as_deref().unwrap()), Some(7));
    }

    #[test]
    fn a_seq_only_difference_is_unchanged() {
        let g = users(vec![list()]);
        let reordered = stamp::with_seq(&g.requests[0].content, 9);
        let p = run(&g, &ex_of(&[("Users/find-all.bru".into(), reordered)]));
        assert_eq!(find(&p, "Users/find-all.bru").action, Action::Unchanged);
    }

    // ------------------------------------------------------------ user changes

    #[test]
    fn a_modified_file_whose_route_did_not_change_is_left_alone_without_conflict() {
        let g = users(vec![list()]);
        let edited = g.requests[0]
            .content
            .replace("auth: inherit", "auth: bearer");
        let p = run(&g, &ex_of(&[("Users/find-all.bru".into(), edited)]));
        assert_eq!(find(&p, "Users/find-all.bru").action, Action::Unchanged);
        assert!(p.entries.iter().all(|e| e.action != Action::Conflict));
    }

    #[test]
    fn a_modified_file_whose_route_changed_is_a_conflict_and_never_updated() {
        let old = users(vec![list()]);
        let edited = old.requests[0]
            .content
            .replace("auth: inherit", "auth: bearer");
        let new = users(vec![with_query(list(), "page")]);
        let p = run(&new, &ex_of(&[("Users/find-all.bru".into(), edited)]));
        let e = find(&p, "Users/find-all.bru");
        assert_eq!(e.action, Action::Conflict);
        assert_eq!(e.conflict, Some(ConflictReason::GeneratedFileModified));
        assert!(e.content.is_none() && e.expected.is_none());
        assert!(!p.blocked, "an ordinary conflict does not block other work");
    }

    #[test]
    fn every_kind_of_user_edit_counts_as_modification() {
        let old = users(vec![list()]);
        let new = users(vec![with_query(list(), "page")]);
        let c = &old.requests[0].content;
        let edits = [
            c.replace("body: none", "body: json"),
            c.replace("  url:", "  url: https://x"),
            c.replace(
                "docs {",
                "script:pre-request {\n  bru.setVar('a', 1);\n}\n\ndocs {",
            ),
            c.replace("  NestJS:", "  My own note\n  NestJS:"),
            c.replace("name: GET /users", "name: Renamed"),
            c.replace('\n', "\r\n"),
            format!("{c}\ntests {{\n  test('x', () => {{}});\n}}\n"),
        ];
        for edited in edits {
            let p = run(
                &new,
                &ex_of(&[("Users/find-all.bru".into(), edited.clone())]),
            );
            assert_eq!(
                find(&p, "Users/find-all.bru").action,
                Action::Conflict,
                "{edited}"
            );
        }
    }

    // ------------------------------------------------------------ stale

    #[test]
    fn unmodified_generated_files_of_removed_routes_are_deleted() {
        let old = users(vec![list(), one()]);
        let new = users(vec![list()]);
        let p = run(&new, &ex_of(&synced(&old)));
        let e = find(&p, "Users/find-one.bru");
        assert_eq!(e.action, Action::Delete);
        assert_eq!(e.expected, Some(content_hash(&old.requests[1].content)));
        assert_eq!(find(&p, "Users/find-all.bru").action, Action::Unchanged);
    }

    #[test]
    fn modified_generated_files_of_removed_routes_are_never_deleted() {
        let old = users(vec![list(), one()]);
        let mut files = synced(&old);
        files[1].1 = files[1].1.replace("auth: inherit", "auth: bearer");
        let p = run(&users(vec![list()]), &ex_of(&files));
        let e = find(&p, "Users/find-one.bru");
        assert_eq!(e.action, Action::Conflict);
        assert_eq!(e.conflict, Some(ConflictReason::StaleGeneratedFileModified));
    }

    #[test]
    fn nothing_is_deleted_when_the_source_analysis_was_incomplete() {
        let old = users(vec![list(), one()]);
        let new = users(vec![list()]);
        let p = plan(
            &Desired {
                requests: &new.requests,
                skipped: &new.skipped,
                complete: false,
            },
            &ex_of(&synced(&old)),
        );
        assert_eq!(find(&p, "Users/find-one.bru").action, Action::Preserve);
        assert!(p.entries.iter().all(|e| e.action != Action::Delete));
        assert!(
            p.diagnostics
                .iter()
                .any(|d| d.message.contains("incomplete"))
        );
    }

    #[test]
    fn nothing_is_deleted_when_existing_files_could_not_be_read() {
        let old = users(vec![list(), one()]);
        let mut ex = ex_of(&synced(&old));
        ex.diagnostics.push(Diagnostic::new(
            Severity::Error,
            None,
            "cannot read directory",
        ));
        let p = run(&users(vec![list()]), &ex);
        assert!(p.entries.iter().all(|e| e.action != Action::Delete));
    }

    #[test]
    fn a_route_whose_path_became_unresolvable_keeps_its_existing_request() {
        // Regression: the skipped route's identity differs from the stamped one
        // (the path changed), so only the anchor can tell they belong together.
        let old = users(vec![list(), one()]);
        let mut dynamic = one();
        dynamic.path = RoutePath::Unresolved;
        let new = users(vec![list(), dynamic]);
        assert_eq!(new.skipped.len(), 1);
        let p = run(&new, &ex_of(&synced(&old)));
        assert_eq!(find(&p, "Users/find-one.bru").action, Action::Preserve);
        assert!(p.entries.iter().all(|e| e.action != Action::Delete));
        assert!(
            p.diagnostics
                .iter()
                .any(|d| d.message.contains("could be generated"))
        );
    }

    #[test]
    fn a_route_that_exists_but_could_not_be_generated_is_not_stale() {
        let old = users(vec![list(), one()]);
        // The route still exists, but its path became unresolved: skipped, not removed.
        let mut unresolved = one();
        unresolved.path = RoutePath::Unresolved;
        let mut changed = one();
        changed.path = RoutePath::Static("/users/:id".into());
        let new = generate(
            &Config::default(),
            &[controller(
                "UsersController",
                "src/users.controller.ts",
                vec![list(), {
                    let mut r = one();
                    r.http_method = HttpMethod::All; // cannot be generated
                    r
                }],
            )],
        );
        assert_eq!(new.skipped.len(), 1);
        let p = run(&new, &ex_of(&synced(&old)));
        // Only the HTTP method changed (to one that cannot be generated); the
        // handler still exists, so its request is kept, not deleted.
        assert_eq!(find(&p, "Users/find-one.bru").action, Action::Preserve);

        // Same identity skipped (e.g. serialization refused): preserved.
        let skipped_same = Generation {
            requests: users(vec![list()]).requests,
            diagnostics: vec![],
            skipped: vec![RouteIdentity::of(
                &controller("UsersController", "src/users.controller.ts", vec![one()]),
                &one(),
            )],
        };
        let p = run(&skipped_same, &ex_of(&synced(&old)));
        assert_eq!(find(&p, "Users/find-one.bru").action, Action::Preserve);
        assert!(
            p.diagnostics
                .iter()
                .any(|d| d.message.contains("could be generated"))
        );
        let _ = (unresolved, changed);
    }

    // ------------------------------------------------------------ unknown / invalid

    #[test]
    fn unknown_files_are_preserved_and_never_matched() {
        let g = users(vec![list()]);
        let ex = existing(&[
            ("Users/find-all.bru", &g.requests[0].content),
            (
                "environments/local.bru",
                "vars {\n  baseUrl: http://localhost\n}\n",
            ),
            ("Users/folder.bru", "meta {\n  name: Users\n}\n"),
            (
                "handwritten.bru",
                "meta {\n  name: Mine\n  type: http\n  seq: 1\n}\n\nget {\n  url: https://x\n}\n",
            ),
        ]);
        let p = run(&g, &ex);
        for path in [
            "environments/local.bru",
            "Users/folder.bru",
            "handwritten.bru",
        ] {
            assert_eq!(find(&p, path).action, Action::Preserve, "{path}");
        }
        assert!(
            p.entries
                .iter()
                .all(|e| e.action != Action::Delete && e.action != Action::Conflict)
        );
    }

    #[test]
    fn invalid_files_are_preserved_and_malformed_metadata_is_a_conflict() {
        let g = users(vec![list()]);
        let ex = existing(&[
            ("a.bru", "binary \u{0} junk"),
            ("b.bru", "docs {\n  NestBruno: v1 id=zz key=00 hash=00\n}\n"),
            (
                "c.bru",
                "docs {\n  NestBruno: v9 id=0123456789abcdef key=0123456789abcdef hash=0123456789abcdef\n}\n",
            ),
        ]);
        let p = run(&g, &ex);
        assert_eq!(find(&p, "a.bru").action, Action::Preserve);
        for path in ["b.bru", "c.bru"] {
            let e = find(&p, path);
            assert_eq!(e.action, Action::Conflict, "{path}");
            assert!(matches!(
                e.conflict,
                Some(ConflictReason::InvalidMetadata(_))
            ));
        }
    }

    // ------------------------------------------------------------ occupied targets / adoption

    #[test]
    fn a_new_request_never_overwrites_an_unknown_file_at_its_path() {
        let g = users(vec![list()]);
        let p = run(&g, &existing(&[("Users/find-all.bru", "my own request\n")]));
        let conflict = p
            .entries
            .iter()
            .find(|e| e.action == Action::Conflict)
            .unwrap();
        assert_eq!(conflict.conflict, Some(ConflictReason::TargetOccupied));
        assert!(
            p.entries
                .iter()
                .all(|e| e.action != Action::Create && e.action != Action::Update)
        );
    }

    #[test]
    fn occupancy_is_detected_case_insensitively() {
        let g = users(vec![list()]);
        let p = run(&g, &existing(&[("users/FIND-ALL.bru", "mine\n")]));
        assert!(
            p.entries
                .iter()
                .any(|e| e.conflict == Some(ConflictReason::TargetOccupied))
        );
        assert!(p.entries.iter().all(|e| e.action != Action::Create));
    }

    #[test]
    fn an_unstamped_file_identical_to_the_generated_output_is_adopted() {
        // What an earlier version (without stamps) wrote.
        let g = users(vec![list()]);
        let stamp_range = g.requests[0]
            .content
            .lines()
            .filter(|l| !l.contains("NestBruno:"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let p = run(&g, &existing(&[("Users/find-all.bru", &stamp_range)]));
        let e = find(&p, "Users/find-all.bru");
        assert_eq!(e.action, Action::Update);
        assert_eq!(e.content.as_deref(), Some(g.requests[0].content.as_str()));
    }

    #[test]
    fn an_unstamped_file_that_differs_even_slightly_is_not_adopted() {
        let g = users(vec![list()]);
        let unstamped = g.requests[0]
            .content
            .lines()
            .filter(|l| !l.contains("NestBruno:"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let edited = unstamped.replace("auth: inherit", "auth: none");
        let p = run(&g, &existing(&[("Users/find-all.bru", &edited)]));
        assert!(
            p.entries
                .iter()
                .any(|e| e.conflict == Some(ConflictReason::TargetOccupied))
        );
        assert!(p.entries.iter().all(|e| e.action != Action::Update));
    }

    /// `find_all` and `findAll` are different handlers (different anchors, and
    /// different paths) that slug to the same file name.
    fn same_file_name_different_route() -> (Generation, Generation) {
        let old = users(vec![route(HttpMethod::Get, "find_all", "/old")]);
        let new = users(vec![route(HttpMethod::Get, "findAll", "/new")]);
        assert_eq!(path_of(&old, 0), path_of(&new, 0));
        (old, new)
    }

    #[test]
    fn a_stale_unmodified_file_in_the_way_is_replaced_in_place() {
        let (old, new) = same_file_name_different_route();
        let p = run(&new, &ex_of(&synced(&old)));
        assert_eq!(p.entries.len(), 1, "{:?}", actions(&p));
        assert_eq!(p.entries[0].action, Action::Update);
        assert!(
            p.entries[0]
                .content
                .as_deref()
                .unwrap()
                .contains("UsersController.findAll")
        );
    }

    #[test]
    fn a_stale_modified_file_in_the_way_is_one_conflict_and_nothing_is_overwritten() {
        let (old, new) = same_file_name_different_route();
        let mut files = synced(&old);
        files[0].1 = files[0].1.replace("auth: inherit", "auth: bearer");
        let p = run(&new, &ex_of(&files));
        assert_eq!(p.entries.len(), 1, "{:?}", actions(&p));
        assert_eq!(p.entries[0].conflict, Some(ConflictReason::TargetOccupied));
    }

    #[test]
    fn a_stale_file_in_the_way_is_not_replaced_when_deletion_is_not_allowed() {
        let (old, new) = same_file_name_different_route();
        let p = plan(
            &Desired {
                requests: &new.requests,
                skipped: &new.skipped,
                complete: false,
            },
            &ex_of(&synced(&old)),
        );
        assert!(
            p.entries
                .iter()
                .all(|e| e.action != Action::Update && e.action != Action::Delete)
        );
        assert!(
            p.entries
                .iter()
                .any(|e| e.conflict == Some(ConflictReason::TargetOccupied))
        );
    }

    // ------------------------------------------------------------ renames and moves

    #[test]
    fn a_handler_rename_updates_the_same_file_in_place() {
        let old = users(vec![list()]);
        let new = users(vec![route(HttpMethod::Get, "listUsers", "/users")]);
        let p = run(&new, &ex_of(&synced(&old)));
        assert_eq!(p.entries.len(), 1, "{:?}", actions(&p));
        let e = &p.entries[0];
        assert_eq!(
            (e.path.to_str(), e.action),
            (Some("Users/find-all.bru"), Action::Update)
        );
        assert!(
            e.content
                .as_deref()
                .unwrap()
                .contains("UsersController.listUsers")
        );
    }

    #[test]
    fn a_handler_rename_of_a_modified_request_is_a_conflict_not_a_duplicate() {
        let old = users(vec![list()]);
        let mut files = synced(&old);
        files[0].1 = files[0].1.replace("auth: inherit", "auth: bearer");
        let new = users(vec![route(HttpMethod::Get, "listUsers", "/users")]);
        let p = run(&new, &ex_of(&files));
        assert_eq!(p.entries.len(), 1);
        assert_eq!(
            p.entries[0].conflict,
            Some(ConflictReason::GeneratedFileModified)
        );
    }

    #[test]
    fn moving_a_controller_file_updates_in_place_instead_of_duplicating() {
        let old = users(vec![list()]);
        let new = generate(
            &Config::default(),
            &[controller(
                "UsersController",
                "src/moved/users.controller.ts",
                vec![list()],
            )],
        );
        let p = run(&new, &ex_of(&synced(&old)));
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].action, Action::Update);
    }

    #[test]
    fn a_changed_path_of_the_same_handler_updates_the_request_in_place() {
        let old = users(vec![one()]);
        let new = users(vec![route(HttpMethod::Get, "findOne", "/users/:userId")]);
        let p = run(&new, &ex_of(&synced(&old)));
        assert_eq!(actions(&p), [("Users/find-one.bru".into(), Action::Update)]);
        assert!(
            p.entries[0]
                .content
                .as_deref()
                .unwrap()
                .contains("/users/:userId")
        );
    }

    #[test]
    fn a_changed_http_method_of_the_same_handler_updates_in_place() {
        let old = users(vec![route(HttpMethod::Get, "save", "/users")]);
        let new = users(vec![route(HttpMethod::Post, "save", "/users")]);
        let p = run(&new, &ex_of(&synced(&old)));
        assert_eq!(actions(&p), [("Users/save.bru".into(), Action::Update)]);
        assert!(
            p.entries[0]
                .content
                .as_deref()
                .unwrap()
                .contains("\npost {")
        );
    }

    #[test]
    fn a_changed_path_of_a_modified_request_is_one_conflict_on_that_file() {
        let old = users(vec![one()]);
        let mut files = synced(&old);
        files[0].1 = files[0].1.replace("auth: inherit", "auth: bearer");
        let new = users(vec![route(HttpMethod::Get, "findOne", "/users/:userId")]);
        let p = run(&new, &ex_of(&files));
        assert_eq!(p.entries.len(), 1, "{:?}", actions(&p));
        assert_eq!(
            p.entries[0].conflict,
            Some(ConflictReason::GeneratedFileModified)
        );
    }

    #[test]
    fn a_name_kept_by_an_in_place_update_does_not_block_a_new_route_that_wants_it() {
        // Regression (found by the property tests). handler1 was renamed to
        // handler3 (the file stayed `handler1.bru`); now a new `handler1` appears.
        let step1 = users(vec![route(HttpMethod::Post, "handler1", "/users/:id")]);
        let step2 = users(vec![route(HttpMethod::Post, "handler3", "/users/:id")]);
        let p2 = run(&step2, &ex_of(&synced(&step1)));
        assert_eq!(
            actions(&p2),
            [("Users/handler1.bru".into(), Action::Update)]
        );
        // Apply step 2: the content of handler3 now lives in handler1.bru.
        let state = vec![(
            "Users/handler1.bru".to_owned(),
            p2.entries[0].content.clone().unwrap(),
        )];

        let step3 = users(vec![
            route(HttpMethod::Get, "handler1", "/users"),
            route(HttpMethod::Get, "handler3", "/users"),
        ]);
        let p3 = run(&step3, &ex_of(&state));
        assert!(
            p3.entries.iter().all(|e| e.action != Action::Conflict),
            "{:?}",
            actions(&p3)
        );
        let creates: Vec<_> = p3
            .entries
            .iter()
            .filter(|e| e.action == Action::Create)
            .collect();
        assert_eq!(creates.len(), 1);
        let name = creates[0].path.to_string_lossy().into_owned();
        assert!(
            name.starts_with("Users/handler1-") && name.ends_with(".bru"),
            "{name}"
        );
        assert_eq!(find(&p3, "Users/handler1.bru").action, Action::Update);
    }

    #[test]
    fn a_file_claimed_by_two_different_routes_is_matched_to_neither() {
        // Regression (found by the property tests). Old: handler2 = POST /health.
        // New: handler2 = GET /users and handler0 = POST /health. The old file
        // matches handler0 by key and handler2 by anchor: ambiguous.
        let old = users(vec![route(HttpMethod::Post, "handler2", "/health")]);
        let new = users(vec![
            route(HttpMethod::Get, "handler2", "/users"),
            route(HttpMethod::Post, "handler0", "/health"),
        ]);
        let p = run(&new, &ex_of(&synced(&old)));
        assert!(
            p.entries.iter().all(|e| e.action != Action::Conflict),
            "{:?}",
            actions(&p)
        );
        // handler2's own file name is free to be reused for handler2; handler0 is new.
        assert_eq!(find(&p, "Users/handler2.bru").action, Action::Update);
        assert_eq!(find(&p, "Users/handler0.bru").action, Action::Create);
        assert!(
            find(&p, "Users/handler2.bru")
                .content
                .as_deref()
                .unwrap()
                .contains("UsersController.handler2")
        );
    }

    #[test]
    fn overloaded_handlers_are_never_matched_by_anchor() {
        // Two routes of one handler name: the anchor is ambiguous, so neither
        // old request is matched to either new one by it.
        let old = users(vec![
            route(HttpMethod::Get, "dup", "/a"),
            route(HttpMethod::Get, "dup", "/b"),
        ]);
        let new = users(vec![
            route(HttpMethod::Get, "dup", "/c"),
            route(HttpMethod::Get, "dup", "/d"),
        ]);
        let p = run(&new, &ex_of(&synced(&old)));
        assert!(
            p.entries
                .iter()
                .all(|e| e.action != Action::Update || e.expected.is_some())
        );
        assert_eq!(
            p.entries
                .iter()
                .filter(|e| e.action == Action::Create)
                .count()
                + p.entries
                    .iter()
                    .filter(|e| e.action == Action::Update)
                    .count(),
            2
        );
    }

    #[test]
    fn a_changed_path_under_a_new_handler_name_creates_and_deletes() {
        let old = users(vec![one()]);
        let new = users(vec![route(HttpMethod::Get, "getUser", "/users/:userId")]);
        let p = run(&new, &ex_of(&synced(&old)));
        assert_eq!(
            p.entries
                .iter()
                .filter(|e| e.action == Action::Create)
                .count(),
            1
        );
        assert_eq!(
            p.entries
                .iter()
                .filter(|e| e.action == Action::Delete)
                .count(),
            1
        );
    }

    #[test]
    fn ambiguous_keys_are_never_matched_by_key() {
        // Two requests of the same controller+method+path (different handlers):
        // after renaming both, the key alone cannot tell which is which.
        let r = |h: &str| route(HttpMethod::Get, h, "/same");
        let old = users(vec![r("a"), r("b")]);
        let new = users(vec![r("c"), r("d")]);
        let p = run(&new, &ex_of(&synced(&old)));
        assert_eq!(
            p.entries
                .iter()
                .filter(|e| e.action == Action::Create)
                .count(),
            2
        );
        assert_eq!(
            p.entries
                .iter()
                .filter(|e| e.action == Action::Delete)
                .count(),
            2
        );
        assert_eq!(
            p.entries
                .iter()
                .filter(|e| e.action == Action::Update)
                .count(),
            0
        );
    }

    // ------------------------------------------------------------ duplicates

    #[test]
    fn duplicate_route_identities_are_critical_and_block_everything() {
        let g = users(vec![list(), list(), one()]);
        let p = run(&g, &Existing::default());
        assert!(p.blocked);
        let dup: Vec<_> = p
            .entries
            .iter()
            .filter(|e| e.conflict == Some(ConflictReason::DuplicateRouteIdentity))
            .collect();
        assert_eq!(dup.len(), 2);
        assert!(
            dup.iter()
                .all(|e| e.conflict.as_ref().is_some_and(ConflictReason::is_critical))
        );
        // The unrelated route is still planned (it is the applier that refuses to run).
        assert_eq!(find(&p, "Users/find-one.bru").action, Action::Create);
    }

    #[test]
    fn two_files_claiming_one_route_are_critical_and_neither_is_chosen() {
        let g = users(vec![list()]);
        let c = &g.requests[0].content;
        let p = run(
            &g,
            &existing(&[("Users/find-all.bru", c), ("Users/find-all-copy.bru", c)]),
        );
        assert!(p.blocked);
        let dup: Vec<_> = p
            .entries
            .iter()
            .filter(|e| e.conflict == Some(ConflictReason::DuplicateBrunoIdentity))
            .collect();
        assert_eq!(dup.len(), 2);
        assert!(
            p.entries
                .iter()
                .all(|e| !matches!(e.action, Action::Update | Action::Delete | Action::Create))
        );
    }

    #[test]
    fn a_stale_duplicate_pair_is_also_reported_and_never_deleted() {
        let old = users(vec![list()]);
        let c = &old.requests[0].content;
        let p = run(
            &users(vec![one()]),
            &existing(&[("A.bru", c), ("B.bru", c)]),
        );
        assert!(p.blocked);
        assert!(p.entries.iter().all(|e| e.action != Action::Delete));
    }

    // ------------------------------------------------------------ properties

    #[test]
    fn the_plan_does_not_depend_on_input_order() {
        let old = users(vec![
            list(),
            one(),
            route(HttpMethod::Post, "create", "/users"),
        ]);
        let new = users(vec![
            with_query(list(), "q"),
            route(HttpMethod::Delete, "remove", "/users/:id"),
        ]);
        let files = synced(&old);
        let mut reversed_files = files.clone();
        reversed_files.reverse();
        let mut reversed_new = Generation {
            requests: new.requests.clone(),
            diagnostics: vec![],
            skipped: vec![],
        };
        reversed_new.requests.reverse();
        let a = run(&new, &ex_of(&files));
        let b = run(&reversed_new, &ex_of(&reversed_files));
        assert_eq!(a, b);
    }

    #[test]
    fn applying_the_plan_and_planning_again_is_a_fixed_point() {
        // Simulate applying: the new state of every file is its planned content.
        let old = users(vec![list(), one()]);
        let new = users(vec![
            with_query(list(), "q"),
            route(HttpMethod::Post, "create", "/users"),
        ]);
        let first = run(&new, &ex_of(&synced(&old)));
        let mut state: Vec<(String, String)> = Vec::new();
        for e in &first.entries {
            let path = e.path.to_string_lossy().into_owned();
            match e.action {
                Action::Create | Action::Update => state.push((path, e.content.clone().unwrap())),
                Action::Delete => {}
                Action::Unchanged => state.push((
                    path.clone(),
                    synced(&old)
                        .into_iter()
                        .find(|(p, _)| *p == path)
                        .unwrap()
                        .1,
                )),
                other => panic!("unexpected {other:?}"),
            }
        }
        let second = run(&new, &ex_of(&state));
        assert!(
            second.entries.iter().all(|e| e.action == Action::Unchanged),
            "{:?}",
            actions(&second)
        );
        assert_eq!(second.entries.len(), new.requests.len());
    }

    #[test]
    fn deleting_never_touches_a_file_that_is_not_proven_generated_and_unmodified() {
        // Property over many mixtures: every Delete/Update target is a file that
        // classify() calls generated and unmodified (or an adopted legacy file).
        let old = users(vec![list(), one()]);
        for edit in [false, true] {
            let mut files = synced(&old);
            files.push(("notes.bru".into(), "hand written\n".into()));
            if edit {
                files[1].1.push_str("\n# edited\n");
            }
            let ex = ex_of(&files);
            let p = run(&users(vec![]), &ex);
            for e in p
                .entries
                .iter()
                .filter(|e| matches!(e.action, Action::Delete | Action::Update))
            {
                let file = ex.files.iter().find(|f| f.path == e.path).unwrap();
                assert!(
                    matches!(
                        file.kind,
                        Kind::Generated {
                            modified: false,
                            ..
                        }
                    ),
                    "{:?}",
                    e.path
                );
            }
            assert_eq!(find(&p, "notes.bru").action, Action::Preserve);
        }
    }
}
