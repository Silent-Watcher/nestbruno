//! Filesystem events: normalization and coalescing.
//!
//! Operating systems report one logical change as several events, and editors
//! save through temporary files and renames. This module turns raw `notify`
//! events into a small [`FsEvent`] vocabulary and collects them into
//! [`Batch`]es with a [`Coalescer`] (debounce).
//!
//! Both parts are pure: no threads, no clocks of their own (the caller passes
//! `Instant`s), no I/O, so they are tested deterministically.
//!
//! A path in a batch is only a *hint that something happened there*. What
//! actually changed is established later by looking at the file system, so the
//! order of events and the distinction between create, modify and rename are
//! deliberately not relied upon.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use notify::EventKind;
use notify::event::{ModifyKind, RenameMode};

/// A normalized filesystem event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    /// Both sides of a rename are known (the OS reported them together).
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
}

impl FsEvent {
    /// Every path the event mentions.
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            Self::Created(p) | Self::Modified(p) | Self::Removed(p) => vec![p.as_path()],
            Self::Renamed { from, to } => vec![from.as_path(), to.as_path()],
        }
    }
}

/// Convert a raw event. Pure reads (`Access`) yield nothing. A rename whose
/// halves arrive separately becomes a removal and a creation; anything not
/// understood is treated as a modification, the conservative choice (the path
/// gets re-checked).
pub fn normalize(event: &notify::Event) -> Vec<FsEvent> {
    let paths = &event.paths;
    match &event.kind {
        EventKind::Access(_) => Vec::new(),
        EventKind::Create(_) => paths.iter().cloned().map(FsEvent::Created).collect(),
        EventKind::Remove(_) => paths.iter().cloned().map(FsEvent::Removed).collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => match paths.as_slice() {
            [from, to] => vec![FsEvent::Renamed {
                from: from.clone(),
                to: to.clone(),
            }],
            _ => paths.iter().cloned().map(FsEvent::Modified).collect(),
        },
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            paths.iter().cloned().map(FsEvent::Removed).collect()
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            paths.iter().cloned().map(FsEvent::Created).collect()
        }
        EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
            paths.iter().cloned().map(FsEvent::Modified).collect()
        }
    }
}

/// What accumulated during one burst.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Batch {
    /// Paths that may have changed, sorted and without duplicates.
    pub paths: BTreeSet<PathBuf>,
    /// Events were lost (queue full, too many paths, or the OS asked for a
    /// rescan): the paths are incomplete and everything must be recomputed.
    pub overflow: bool,
}

/// Debouncer: collects paths until the burst is over.
///
/// A batch is ready when no event arrived for `debounce`, or when the first
/// event of the burst is `max_wait` old (so a steady stream cannot starve
/// processing). Memory is bounded: past `limit` distinct paths the batch
/// degrades to an overflow marker instead of growing.
#[derive(Debug)]
pub struct Coalescer {
    limit: usize,
    debounce: Duration,
    max_wait: Duration,
    batch: Batch,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Coalescer {
    pub fn new(debounce: Duration, max_wait: Duration, limit: usize) -> Self {
        Self {
            limit,
            debounce,
            max_wait,
            batch: Batch::default(),
            first: None,
            last: None,
        }
    }

    /// Record paths seen at `now`.
    pub fn push(&mut self, paths: impl IntoIterator<Item = PathBuf>, now: Instant) {
        for path in paths {
            if self.batch.overflow {
                break;
            }
            self.batch.paths.insert(path);
            if self.batch.paths.len() > self.limit {
                self.mark_overflow(now);
            }
        }
        self.touch(now);
    }

    /// Record that events were lost.
    pub fn mark_overflow(&mut self, now: Instant) {
        self.batch.overflow = true;
        // The individual paths no longer matter; release them.
        self.batch.paths.clear();
        self.touch(now);
    }

    fn touch(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// Whether anything is waiting.
    pub fn is_pending(&self) -> bool {
        self.first.is_some()
    }

    /// When the pending batch becomes ready; `None` if nothing is pending.
    pub fn deadline(&self) -> Option<Instant> {
        let (first, last) = (self.first?, self.last?);
        Some((last + self.debounce).min(first + self.max_wait))
    }

    /// Whether the pending batch is ready at `now`.
    pub fn is_ready(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|deadline| now >= deadline)
    }

    /// Take the pending batch and start over.
    pub fn take(&mut self) -> Batch {
        self.first = None;
        self.last = None;
        std::mem::take(&mut self.batch)
    }
}

#[cfg(test)]
mod tests {
    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, RemoveKind};

    use super::*;

    fn event(kind: EventKind, paths: &[&str]) -> notify::Event {
        let mut e = notify::Event::new(kind);
        for p in paths {
            e = e.add_path(PathBuf::from(p));
        }
        e
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn creations_modifications_and_removals_are_normalized() {
        assert_eq!(
            normalize(&event(EventKind::Create(CreateKind::File), &["/a"])),
            [FsEvent::Created(p("/a"))]
        );
        assert_eq!(
            normalize(&event(EventKind::Create(CreateKind::Folder), &["/d"])),
            [FsEvent::Created(p("/d"))]
        );
        assert_eq!(
            normalize(&event(EventKind::Remove(RemoveKind::File), &["/a"])),
            [FsEvent::Removed(p("/a"))]
        );
        for kind in [
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Any,
            EventKind::Other,
        ] {
            assert_eq!(
                normalize(&event(kind, &["/a"])),
                [FsEvent::Modified(p("/a"))],
                "{kind:?}"
            );
        }
    }

    #[test]
    fn access_events_are_ignored() {
        assert!(normalize(&event(EventKind::Access(AccessKind::Any), &["/a"])).is_empty());
    }

    #[test]
    fn renames_with_both_paths_are_renames() {
        let e = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/old", "/new"],
        );
        assert_eq!(
            normalize(&e),
            [FsEvent::Renamed {
                from: p("/old"),
                to: p("/new")
            }]
        );
        assert_eq!(
            normalize(&e)[0].paths(),
            [Path::new("/old"), Path::new("/new")]
        );
    }

    #[test]
    fn split_renames_become_removal_and_creation() {
        let from = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            &["/old"],
        );
        let to = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            &["/new"],
        );
        assert_eq!(normalize(&from), [FsEvent::Removed(p("/old"))]);
        assert_eq!(normalize(&to), [FsEvent::Created(p("/new"))]);
    }

    #[test]
    fn unclear_renames_are_checked_conservatively() {
        let any = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            &["/x"],
        );
        assert_eq!(normalize(&any), [FsEvent::Modified(p("/x"))]);
        // "Both" with an unexpected number of paths must not panic or lose paths.
        let odd = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/a", "/b", "/c"],
        );
        assert_eq!(normalize(&odd).len(), 3);
        assert!(
            normalize(&event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &[]
            ))
            .is_empty()
        );
    }

    // ------------------------------------------------------------ coalescer

    const DEBOUNCE: Duration = Duration::from_millis(100);
    const MAX_WAIT: Duration = Duration::from_millis(1000);

    fn coalescer(limit: usize) -> Coalescer {
        Coalescer::new(DEBOUNCE, MAX_WAIT, limit)
    }

    #[test]
    fn nothing_pending_means_no_deadline() {
        let mut c = coalescer(10);
        assert!(!c.is_pending() && c.deadline().is_none() && !c.is_ready(Instant::now()));
        assert_eq!(c.take(), Batch::default());
    }

    #[test]
    fn a_burst_becomes_one_batch_after_the_quiet_period() {
        let t0 = Instant::now();
        let mut c = coalescer(10);
        // write, modify, close, rename, modify of one save: five events, one path
        for ms in [0, 5, 9, 12, 20] {
            c.push([p("/src/a.ts")], t0 + Duration::from_millis(ms));
        }
        assert!(
            !c.is_ready(t0 + Duration::from_millis(119)),
            "still inside the quiet period of the last event"
        );
        assert!(c.is_ready(t0 + Duration::from_millis(120)));
        let batch = c.take();
        assert_eq!(batch.paths.len(), 1);
        assert!(!batch.overflow && !c.is_pending());
    }

    #[test]
    fn every_new_event_restarts_the_quiet_period() {
        let t0 = Instant::now();
        let mut c = coalescer(10);
        c.push([p("/a")], t0);
        c.push([p("/b")], t0 + Duration::from_millis(90));
        assert_eq!(c.deadline(), Some(t0 + Duration::from_millis(190)));
    }

    #[test]
    fn a_steady_stream_cannot_postpone_processing_forever() {
        let t0 = Instant::now();
        let mut c = coalescer(10_000);
        for ms in (0..2000).step_by(50) {
            c.push([p(&format!("/f{ms}"))], t0 + Duration::from_millis(ms));
        }
        assert_eq!(
            c.deadline(),
            Some(t0 + MAX_WAIT),
            "capped by the first event of the burst"
        );
        assert!(c.is_ready(t0 + MAX_WAIT));
    }

    #[test]
    fn duplicates_collapse_and_paths_are_sorted() {
        let t0 = Instant::now();
        let mut c = coalescer(10);
        c.push([p("/b"), p("/a"), p("/b"), p("/a")], t0);
        let paths: Vec<_> = c.take().paths.into_iter().collect();
        assert_eq!(paths, [p("/a"), p("/b")]);
    }

    #[test]
    fn memory_is_bounded_by_degrading_to_overflow() {
        let t0 = Instant::now();
        let mut c = coalescer(3);
        c.push((0..100).map(|i| p(&format!("/f{i}"))), t0);
        let batch = c.take();
        assert!(batch.overflow);
        assert!(
            batch.paths.is_empty(),
            "the paths are released once they no longer matter"
        );

        c.push([p("/a")], t0);
        assert!(
            !c.take().overflow,
            "taking a batch resets the overflow state"
        );
    }

    #[test]
    fn lost_events_are_marked_and_still_debounced() {
        let t0 = Instant::now();
        let mut c = coalescer(10);
        c.push([p("/a")], t0);
        c.mark_overflow(t0 + Duration::from_millis(10));
        assert!(c.is_pending() && !c.is_ready(t0 + Duration::from_millis(50)));
        assert!(c.is_ready(t0 + Duration::from_millis(110)));
        let batch = c.take();
        assert!(batch.overflow && batch.paths.is_empty());
    }

    #[test]
    fn a_batch_is_taken_exactly_once() {
        let t0 = Instant::now();
        let mut c = coalescer(10);
        c.push([p("/a")], t0);
        assert_eq!(c.take().paths.len(), 1);
        assert_eq!(c.take(), Batch::default());
        assert!(c.deadline().is_none());
    }
}
