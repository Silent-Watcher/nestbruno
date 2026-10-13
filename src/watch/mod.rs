//! Watch mode: keep the Bruno collection in sync while the sources change.
//!
//! ```text
//! notify callback ─▶ bounded channel ─▶ loop ─▶ Coalescer (debounce)
//!                                                   │ batch of dirty paths
//!                                                   ▼
//!                              Workspace::refresh (incremental parse + sync)
//!                                                   │
//!                                                   ▼
//!                                         Notice ─▶ sink (terminal, tests)
//! ```
//!
//! # Concurrency model
//!
//! One loop thread owns all state. The only other thread is `notify`'s, whose
//! callback does a non-blocking `try_send` into a bounded channel; if the
//! channel is full the event is dropped and an overflow flag makes the next
//! batch a full recomputation. Processing is strictly sequential: events that
//! arrive while a batch is being processed wait in the channel and form the
//! next batch (coalesced), so there is no overlapping synchronization and no
//! unbounded queue. The loop sleeps in `recv_timeout` until the debounce
//! deadline; there is no polling.
//!
//! # What is watched
//!
//! Every directory of the source tree that discovery would enter (one
//! non-recursive watch each; `.git`, `node_modules`, configured excludes and
//! over-deep trees are never watched), plus the project root non-recursively
//! for `nestbruno.toml`. The collection directory is **not** watched for
//! source changes, which rules out feedback loops: our own writes never
//! trigger anything. Directories that appear later get watches when their
//! creation is seen, *before* they are scanned.
//!
//! # Fallback to a full recomputation
//!
//! Whenever incremental correctness cannot be argued the whole state is
//! rebuilt from disk: lost events (queue overflow, OS rescan request, too many
//! pending paths), a watcher error, or an incremental step that failed. A
//! changed `nestbruno.toml` restarts the session with the new configuration
//! (an invalid one is reported and the previous configuration stays active).
//!
//! # Shutdown
//!
//! [`Stopper::stop`] wakes the loop. A batch in progress finishes first (every
//! file write is atomic), then the watcher is dropped and the loop returns.

pub mod events;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::CONFIG_FILE_NAME;
use crate::diag::{Diagnostic, Severity};
use crate::discovery::{Matcher, contained_source_root};
use crate::error::Error;
use crate::project::Project;
use crate::workspace::{Delta, Workspace};
use events::{Batch, Coalescer, normalize};

/// Capacity of the event channel.
const CHANNEL_CAPACITY: usize = 4096;
/// More distinct paths than this in one burst mean "something big happened".
const MAX_PENDING_PATHS: usize = 10_000;

/// Watch settings.
#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// Quiet time after the last event before a batch is processed.
    pub debounce: Duration,
}

impl WatchOptions {
    pub const DEFAULT_DEBOUNCE_MS: u64 = 200;

    /// Longest a continuous stream of events may postpone processing.
    fn max_wait(&self) -> Duration {
        (self.debounce * 10).max(Duration::from_secs(2))
    }
}

/// Something the watch loop wants to tell the outside world. Contains data
/// only; presentation is up to the consumer.
#[derive(Debug)]
pub enum Notice {
    /// The initial full synchronization.
    Initial(Delta),
    /// Watches are in place.
    Ready {
        /// The watched source directory, relative to the project.
        source: PathBuf,
        directories: usize,
    },
    /// A batch of changes was processed.
    Synced(Delta),
    /// Everything was recomputed from disk.
    Fallback { reason: String, delta: Delta },
    /// `nestbruno.toml` changed and was applied.
    ConfigReloaded,
    /// A recoverable problem.
    Problem(Diagnostic),
    /// The loop ended.
    Stopped,
}

enum Msg {
    Event(notify::Result<notify::Event>),
    Stop,
}

/// Asks a running [`Watch`] to stop. Cheap to clone; safe to call from a
/// signal-handler thread.
#[derive(Clone)]
pub struct Stopper {
    tx: SyncSender<Msg>,
    flag: Arc<AtomicBool>,
}

impl Stopper {
    pub fn stop(&self) {
        // The flag is authoritative; the message only wakes the loop (and may
        // be dropped if the channel is full, which is harmless).
        self.flag.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(Msg::Stop);
    }
}

/// A watch session.
pub struct Watch {
    tx: SyncSender<Msg>,
    rx: Receiver<Msg>,
    stop: Arc<AtomicBool>,
}

impl Default for Watch {
    fn default() -> Self {
        Self::new()
    }
}

enum End {
    Stop,
    Reload(Box<Project>),
}

impl Watch {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel(CHANNEL_CAPACITY);
        Self {
            tx,
            rx,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn stopper(&self) -> Stopper {
        Stopper {
            tx: self.tx.clone(),
            flag: Arc::clone(&self.stop),
        }
    }

    /// Run until stopped. Fatal problems (cannot watch, configuration that
    /// cannot be loaded at start) are errors; everything else is a [`Notice`].
    pub fn run(
        self,
        project: Project,
        options: &WatchOptions,
        sink: &mut dyn FnMut(Notice),
    ) -> Result<(), Error> {
        let mut project = project;
        loop {
            match self.session(&project, options, sink)? {
                End::Stop => {
                    sink(Notice::Stopped);
                    return Ok(());
                }
                End::Reload(new) => {
                    project = *new;
                    sink(Notice::ConfigReloaded);
                }
            }
        }
    }

    fn session(
        &self,
        project: &Project,
        options: &WatchOptions,
        sink: &mut dyn FnMut(Notice),
    ) -> Result<End, Error> {
        let source_root = contained_source_root(project)?;
        let matcher = Matcher::new(project.config())?;
        let overflow = Arc::new(AtomicBool::new(false));

        let tx = self.tx.clone();
        let lost = Arc::clone(&overflow);
        let mut watcher = notify::recommended_watcher(move |result| {
            if tx.try_send(Msg::Event(result)).is_err() {
                lost.store(true, Ordering::SeqCst);
            }
        })
        .map_err(watch_error)?;

        // Watch first, analyze second: whatever happens during the initial
        // analysis is queued and processed afterwards.
        watcher
            .watch(project.root(), RecursiveMode::NonRecursive)
            .map_err(watch_error)?;
        let mut problems = Vec::new();
        let skip = collection_inside_source(project);
        let directories = register_tree(
            &mut watcher,
            &matcher,
            &source_root,
            Path::new(""),
            skip.as_deref(),
            &mut problems,
        )?;
        for problem in problems {
            sink(Notice::Problem(problem));
        }

        let (mut workspace, initial) = Workspace::start(project.clone())?;
        sink(Notice::Initial(initial));
        sink(Notice::Ready {
            source: project.config().source.clone(),
            directories,
        });

        let config_path = project.root().join(CONFIG_FILE_NAME);
        let mut coalescer = Coalescer::new(options.debounce, options.max_wait(), MAX_PENDING_PATHS);

        loop {
            if overflow.swap(false, Ordering::SeqCst) {
                coalescer.mark_overflow(Instant::now());
            }
            let message = match coalescer.deadline() {
                Some(deadline) => {
                    match self
                        .rx
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    {
                        Ok(message) => Some(message),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return Ok(End::Stop),
                    }
                }
                None => match self.rx.recv() {
                    Ok(message) => Some(message),
                    Err(_) => return Ok(End::Stop),
                },
            };
            if self.stop.load(Ordering::SeqCst) {
                return Ok(End::Stop);
            }

            match message {
                Some(Msg::Stop) => return Ok(End::Stop),
                Some(Msg::Event(Ok(event))) => {
                    let now = Instant::now();
                    if event.need_rescan() {
                        coalescer.mark_overflow(now);
                    }
                    let mut keep = Vec::new();
                    for fs_event in normalize(&event) {
                        for path in fs_event.paths() {
                            // The configuration file travels through the same
                            // debounce as everything else: reading it while an
                            // editor is still writing it would see a truncated
                            // (and possibly valid-but-empty) file.
                            if path == config_path || worth_checking(&workspace, project, path) {
                                keep.push(path.to_owned());
                            }
                        }
                    }
                    coalescer.push(keep, now);
                }
                Some(Msg::Event(Err(err))) => {
                    if matches!(err.kind, notify::ErrorKind::MaxFilesWatch) {
                        return Err(watch_error(err));
                    }
                    sink(Notice::Problem(Diagnostic::new(
                        Severity::Error,
                        None,
                        format!("file watcher error: {err}; recomputing everything"),
                    )));
                    coalescer.mark_overflow(Instant::now());
                }
                None => {}
            }

            if coalescer.is_ready(Instant::now()) {
                let mut batch = coalescer.take();
                // A configuration change restarts the session with the new
                // configuration; an invalid one is reported and ignored. Lost
                // events may have hidden a change, so check then as well.
                if batch.paths.remove(&config_path) || batch.overflow {
                    match Project::open(project.root()) {
                        Ok(new) if new.config() != project.config() => {
                            return Ok(End::Reload(Box::new(new)));
                        }
                        Ok(_) => {}
                        Err(err) => sink(Notice::Problem(Diagnostic::new(
                            Severity::Error,
                            Some(PathBuf::from(CONFIG_FILE_NAME)),
                            format!("{err}; keeping the previous configuration"),
                        ))),
                    }
                }
                // Problems met while registering new directories are collected
                // (the sink is busy with the batch) and reported right after.
                let mut register_problems: Vec<Diagnostic> = Vec::new();
                {
                    let mut register = |dir: &Path| {
                        let relative = dir
                            .strip_prefix(&source_root)
                            .map(Path::to_path_buf)
                            .unwrap_or_default();
                        if let Err(err) = register_tree(
                            &mut watcher,
                            &matcher,
                            &source_root,
                            &relative,
                            skip.as_deref(),
                            &mut register_problems,
                        ) {
                            register_problems.push(Diagnostic::new(
                                Severity::Error,
                                Some(dir.to_owned()),
                                err.to_string(),
                            ));
                        }
                    };
                    process_batch(&mut workspace, batch, &mut register, sink)?;
                }
                for problem in register_problems {
                    sink(Notice::Problem(problem));
                }
            }
        }
    }
}

/// Process one batch. Free of any OS watcher so it can be tested directly;
/// `register` is called for every directory that needs watches.
pub fn process_batch(
    workspace: &mut Workspace,
    batch: Batch,
    register: &mut dyn FnMut(&Path),
    sink: &mut dyn FnMut(Notice),
) -> Result<(), Error> {
    if batch.overflow {
        let delta = workspace.resync()?;
        sink(Notice::Fallback {
            reason: "file system events were lost".to_owned(),
            delta,
        });
        return Ok(());
    }

    let root = workspace.project().root().to_owned();
    let mut dirty = BTreeSet::new();
    for absolute in &batch.paths {
        let Ok(relative) = absolute.strip_prefix(&root) else {
            continue;
        };
        // New directories get their watches before they are scanned, so that
        // nothing created inside them in between is missed.
        if fs::symlink_metadata(absolute).is_ok_and(|meta| meta.is_dir()) {
            register(absolute);
        }
        dirty.insert(relative.to_owned());
    }

    match workspace.refresh(&dirty) {
        Ok(delta) => {
            if is_reportable(&delta) {
                sink(Notice::Synced(delta));
            }
        }
        Err(err) => {
            sink(Notice::Problem(Diagnostic::new(
                Severity::Error,
                None,
                format!("incremental update failed ({err}); recomputing everything"),
            )));
            let delta = workspace.resync()?;
            sink(Notice::Fallback {
                reason: err.to_string(),
                delta,
            });
        }
    }
    Ok(())
}

/// Whether a step is worth telling anyone about.
fn is_reportable(delta: &Delta) -> bool {
    !delta.changed_files.is_empty()
        || delta.result.has_changes()
        || !delta.result.conflicts.is_empty()
        || !delta.result.failed.is_empty()
        || delta
            .diagnostics
            .iter()
            .any(|d| d.severity != Severity::Note)
}

/// Cheap pre-filter for raw event paths (absolute): source files that could be
/// controllers, and paths that might be directories (no extension). Anything
/// under the collection directory is ignored, which prevents feedback loops.
fn worth_checking(workspace: &Workspace, project: &Project, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(project.root()) else {
        return false;
    };
    workspace.is_relevant(relative)
        || (relative.extension().is_none() && workspace.is_watchable_dir(relative))
}

/// Watch `relative` (relative to the source directory) and every directory
/// below it that discovery would enter. Returns how many directories were
/// registered. Unreadable directories are reported, not fatal.
fn register_tree(
    watcher: &mut RecommendedWatcher,
    matcher: &Matcher,
    source_root: &Path,
    relative: &Path,
    skip: Option<&Path>,
    problems: &mut Vec<Diagnostic>,
) -> Result<usize, Error> {
    let mut count = 0;
    let mut pending = vec![relative.to_owned()];
    while let Some(rel) = pending.pop() {
        // Pruned by discovery's rules, or the collection itself (when it lies
        // inside the source tree): output, not input.
        if matcher.is_pruned_dir(&rel) || skip.is_some_and(|s| rel.starts_with(s)) {
            continue;
        }
        let absolute = source_root.join(&rel);
        match watcher.watch(&absolute, RecursiveMode::NonRecursive) {
            Ok(()) => count += 1,
            // It disappeared while we were looking; its parent's event covers it.
            Err(err) if matches!(err.kind, notify::ErrorKind::PathNotFound) => continue,
            Err(err) if matches!(err.kind, notify::ErrorKind::MaxFilesWatch) => {
                return Err(watch_error(err));
            }
            Err(err) => {
                problems.push(Diagnostic::new(
                    Severity::Error,
                    Some(absolute.clone()),
                    format!("cannot watch this directory: {err}"),
                ));
                continue;
            }
        }
        let Ok(entries) = fs::read_dir(&absolute) else {
            continue;
        };
        for entry in entries.flatten() {
            // `file_type` does not follow symlinks: they are never watched.
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(rel.join(entry.file_name()));
            }
        }
    }
    Ok(count)
}

/// The collection directory relative to the source directory, if inside it.
fn collection_inside_source(project: &Project) -> Option<PathBuf> {
    let clean = |path: &Path| -> PathBuf {
        path.components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .collect()
    };
    clean(&project.config().bruno)
        .strip_prefix(clean(&project.config().source))
        .ok()
        .map(Path::to_path_buf)
}

fn watch_error(err: notify::Error) -> Error {
    if matches!(err.kind, notify::ErrorKind::MaxFilesWatch) {
        Error::Watch(
            "the operating system limit on watched files/directories was reached (on Linux raise fs.inotify.max_user_watches), or point `source` at a smaller directory".to_owned(),
        )
    } else {
        Error::Watch(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(name: &str, route: &str) -> String {
        format!(
            "@Controller('{name}') export class {name}Controller {{ @Get('{route}') find{name}() {{}} }}"
        )
    }

    fn setup() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let full = dir.path().join("src/a/a.controller.ts");
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, controller("a", "x")).unwrap();
        let project = Project::open(dir.path()).unwrap();
        let (workspace, _) = Workspace::start(project).unwrap();
        (dir, workspace)
    }

    fn batch(root: &Path, paths: &[&str]) -> Batch {
        Batch {
            paths: paths.iter().map(|p| root.join(p)).collect(),
            overflow: false,
        }
    }

    fn run(workspace: &mut Workspace, batch: Batch) -> (Vec<String>, Vec<PathBuf>) {
        let mut notices = Vec::new();
        let mut registered = Vec::new();
        process_batch(
            workspace,
            batch,
            &mut |dir| registered.push(dir.to_owned()),
            &mut |notice| {
                notices.push(match notice {
                    Notice::Synced(d) => format!("synced:{}", d.changed_files.len()),
                    Notice::Fallback { .. } => "fallback".to_owned(),
                    Notice::Problem(_) => "problem".to_owned(),
                    other => format!("{other:?}"),
                });
            },
        )
        .unwrap();
        (notices, registered)
    }

    #[test]
    fn a_change_is_reported_once() {
        let (dir, mut ws) = setup();
        fs::write(
            dir.path().join("src/a/a.controller.ts"),
            controller("a", "changed"),
        )
        .unwrap();
        let (notices, registered) = run(&mut ws, batch(dir.path(), &["src/a/a.controller.ts"]));
        assert_eq!(notices, ["synced:1"]);
        assert!(registered.is_empty(), "a file is not a directory to watch");
    }

    #[test]
    fn nothing_relevant_means_no_notice() {
        let (dir, mut ws) = setup();
        fs::write(dir.path().join("src/a/readme.txt"), "x").unwrap();
        let (notices, _) = run(
            &mut ws,
            batch(
                dir.path(),
                &["src/a/readme.txt", "src/ghost.ts", "outside.ts"],
            ),
        );
        assert!(notices.is_empty(), "{notices:?}");
    }

    #[test]
    fn paths_outside_the_project_are_ignored() {
        let (_dir, mut ws) = setup();
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("x.controller.ts"), controller("x", "y")).unwrap();
        let outside = Batch {
            paths: [other.path().join("x.controller.ts")].into_iter().collect(),
            overflow: false,
        };
        let (notices, registered) = run(&mut ws, outside);
        assert!(notices.is_empty() && registered.is_empty());
    }

    #[test]
    fn new_directories_are_registered_before_they_are_scanned() {
        let (dir, mut ws) = setup();
        let new = dir.path().join("src/new/deep");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("n.controller.ts"), controller("n", "q")).unwrap();
        let (notices, registered) = run(&mut ws, batch(dir.path(), &["src/new"]));
        assert_eq!(registered, [dir.path().join("src/new")]);
        assert_eq!(notices, ["synced:1"]);
    }

    #[test]
    fn lost_events_trigger_a_full_recomputation() {
        let (dir, mut ws) = setup();
        // Changes the lost events would have reported:
        fs::write(
            dir.path().join("src/a/b.controller.ts"),
            controller("b", "z"),
        )
        .unwrap();
        fs::remove_file(dir.path().join("src/a/a.controller.ts")).unwrap();
        let (notices, _) = run(
            &mut ws,
            Batch {
                paths: BTreeSet::new(),
                overflow: true,
            },
        );
        assert_eq!(notices, ["fallback"]);
        assert!(
            dir.path().join("bruno/b/findb.bru").is_file(),
            "the new file was found by the rescan"
        );
        assert!(
            !dir.path().join("bruno/a").exists(),
            "and the removed one was noticed"
        );
    }

    #[test]
    fn relevance_filtering_ignores_the_collection_and_unrelated_paths() {
        let (dir, ws) = setup();
        let project = ws.project().clone();
        let root = dir.path().canonicalize().unwrap();
        for ignored in [
            "bruno/Users/x.bru",
            "bruno/x",
            "README.md",
            "src/a/data.json",
            "src/node_modules/x.ts",
            "../outside.ts",
        ] {
            assert!(
                !worth_checking(&ws, &project, &root.join(ignored)),
                "{ignored}"
            );
        }
        for kept in ["src/a/a.controller.ts", "src/new-dir", "src/a/deeper"] {
            assert!(worth_checking(&ws, &project, &root.join(kept)), "{kept}");
        }
    }

    #[test]
    fn reportable_steps_are_only_the_interesting_ones() {
        assert!(!is_reportable(&Delta::default()));
        let mut changed = Delta::default();
        changed.changed_files.push(PathBuf::from("src/a.ts"));
        assert!(is_reportable(&changed));
        let mut note_only = Delta::default();
        note_only
            .diagnostics
            .push(Diagnostic::new(Severity::Note, None, "fyi"));
        assert!(!is_reportable(&note_only));
        let mut warning = Delta::default();
        warning
            .diagnostics
            .push(Diagnostic::new(Severity::Warning, None, "careful"));
        assert!(is_reportable(&warning));
    }

    #[test]
    fn the_stop_flag_is_authoritative_even_if_the_wakeup_message_is_lost() {
        let watch = Watch::new();
        let stopper = watch.stopper();
        // Fill the channel so the wake-up message cannot be queued.
        while watch.tx.try_send(Msg::Stop).is_ok() {}
        stopper.stop();
        assert!(watch.stop.load(Ordering::SeqCst));
    }
}
