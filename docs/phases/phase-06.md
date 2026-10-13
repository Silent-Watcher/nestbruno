# Phase 6: Watch mode and incremental synchronization

## Objective

`nestbruno watch`: keep the Bruno collection synchronized while the sources
change, re-analyzing only what changed, without ever acting on a partial view
and without feedback loops.

## Scope

Implemented: filesystem watching, relevance filtering, debounce/coalescing,
incremental analysis and generation, incremental synchronization, fallbacks to
a full recomputation, configuration reload, error recovery, graceful shutdown,
watch output, tests. Not implemented: Git integration, persistent caches,
daemons, remote anything.

## Architecture

| Module | Role |
| --- | --- |
| `watch/events.rs` | Pure: `FsEvent` normalization of `notify` events; `Coalescer` (debounce, max latency, bounded). Time is injected, so it is tested without sleeping |
| `watch/mod.rs` | The loop (`Watch`, `Stopper`), per-directory watch registration, `process_batch` (testable without an OS watcher), `Notice` |
| `workspace.rs` | `Workspace`: per-file analysis state, per-controller generation cache, in-memory collection index; `start`, `refresh(dirty)`, `resync` |
| `discovery.rs` | `Matcher::is_relevant` / `is_pruned_dir` (the single definition of relevance), `discover_in` (scan one subtree) |
| `nest/mod.rs` | `analyze_each` (per-file results) |
| `bruno/mod.rs` | `folder_names` (global, cheap) split from `generate_controller` (per controller, cacheable) |
| `sync/*` | Unchanged rules; `existing::reload` re-reads one file |

Layering is preserved: the watcher knows nothing about NestJS or Bruno; the
workspace knows nothing about notify; the CLI only renders `Notice`s.

## Watcher

- One non-recursive watch per directory that discovery would enter, so
  `.git`, `node_modules`, configured excludes, over-deep trees, **and the
  collection directory** are never watched (this also holds when the
  collection lies inside the source tree, `source = "."`). The project root is
  watched non-recursively for `nestbruno.toml`. Symlinks are never followed.
- Directories that appear later get watches when their creation is seen,
  *before* they are scanned, so nothing created inside in between is missed.
- Watch limits (`fs.inotify.max_user_watches`) produce a clear fatal error.

## Event normalization and debounce

Raw events become `Created / Modified / Removed / Renamed`; access events are
dropped; split rename halves become removal + creation; anything unclear is a
modification. **Events are only hints**: a path becomes *dirty*, and processing
re-stats and re-reads it, so ordering, duplication and rename choreography of
editors (write temp, rename over, touch) cannot misrepresent the state.

The `Coalescer` makes a batch ready when no event arrived for `--debounce`
(default 200 ms, 10 to 60,000), or when the first event of the burst is
`max(10 × debounce, 2 s)` old (a steady stream cannot starve processing).
Duplicates collapse, paths are sorted, memory is bounded (more than 10,000
paths degrade to an *overflow* marker). The configuration file travels through
the same debounce.

## Incremental analysis and generation

`Workspace` keeps, per source file, the controllers of its last successful
analysis and the generated requests per controller. `refresh(dirty)`:

1. stats each dirty path: gone (file or whole directory) removes the state of
   everything below it; a file is re-analyzed if relevant; a new directory is
   scanned (`discover_in`); symlinks and special files are ignored;
2. only those files are read and parsed (one parse each, on the large-stack
   worker of phase 3);
3. generation is recomputed only for controllers whose source changed or whose
   *folder name* changed (folder names are made distinct across all
   controllers, the only global part; it is cheap and recomputed);
4. an event that changes nothing relevant returns immediately.

A file that cannot be read or parsed **keeps its last good routes** and marks
the state incomplete: while any file is in that state, nothing is deleted (the
phase 5 rule), so an editor's half-saved file destroys nothing.

## Affected routes

`routes_affected` counts distinct handlers whose route was added, removed or
changed in identity or parameters between the old and new state of the
changed files (source line numbers are ignored: inserting a line moves no
request).

## Synchronization

The same pure `reconcile::plan` as `sync`, run over the in-memory desired
state and in-memory index of the collection; only planned `Create / Update /
Delete` operations touch the disk (through the verified writer). This keeps
one authoritative rule set; a property test shows that any history of changes
ends in the same collection as a fresh `sync` of the final sources.

Because the index can be stale (the user edits Bruno files while watching),
every replace/delete re-verifies the file hash; on a mismatch the failed paths
are re-read from disk and the plan is recomputed once. A request edited in
Bruno meanwhile therefore becomes a conflict (reported once, not on every later
event) and is never overwritten.

Renames and moves need no special case: both paths are dirty; the old one is
gone, the new one parses; the phase 5 matching (`key`, `anchor`) updates the
requests in place without duplicates.

## Fallback to a full recomputation

Everything is rebuilt from disk (`resync`) when: events were lost (channel
full, 10,000+ paths, the OS requested a rescan); the watcher reported an error;
an incremental step itself failed. A changed, valid `nestbruno.toml` restarts
the session (new watches, full start) with the new configuration; an invalid
one is reported and the previous configuration stays active.

## Concurrency model

One loop thread owns all state; `notify`'s thread only `try_send`s into a
bounded channel (overflow sets a flag). Batches are processed strictly one at a
time; events arriving meanwhile wait in the channel and form the next
(coalesced) batch. The loop sleeps in `recv_timeout` until the debounce
deadline: no polling, no unbounded queues, no overlapping synchronization.

## Error recovery and shutdown

Parse errors, unreadable files, conflicts and failed operations are reported and
watching continues; only startup problems (missing source, cannot watch,
invalid configuration at start) are fatal. SIGINT, SIGTERM, SIGHUP (and Ctrl-C
on Windows) wake the loop, which finishes the batch in progress (all writes are
atomic), drops the watcher and exits 0 after printing `Stopped.`; a closed
stdout also stops it.

## Output

```text
Initial sync:
  Created: 4  Updated: 0  Deleted: 0  Conflicts: 0  Unchanged: 0
Watching src (3 directories). Press Ctrl-C to stop.
Changed: src/users/users.controller.ts
  Routes affected: 1
  Created: 0  Updated: 1  Deleted: 0  Conflicts: 0
  CONFLICT users/find-one.bru: <reason>
    what to do: <suggestion>
```

Diagnostics go to stderr; `-v` adds notes. The terminal layer only renders
`Notice` values.

## Performance

Measured (release build, Linux sandbox; controllers of 100 routes each, every
route with one query and one body parameter; one run of one program timing
`Workspace` calls):

| Routes | full start (writes all) | 1 file, 100 requests change | file re-saved unchanged | 1 route edited | unrelated file | full resync (nothing to write) |
| --- | --- | --- | --- | --- | --- | --- |
| 500 | 105 ms | 27 ms | 2 ms | 3 ms | 0.10 ms | 9 ms |
| 5,000 | 990 ms | 36 ms | 9 ms | 9 ms | 0.09 ms | 102 ms |
| 20,000 | 4,544 ms | 129 ms | 89 ms | 68 ms | 0.18 ms | 426 ms |

**These numbers are noisy.** Earlier runs of the same program on the same
machine differed by up to about 2x (for example 105 ms instead of 68 ms for one
edited route at 20,000 routes, and 124 ms before an optimization described
below), so read them as orders of magnitude and ratios, not as exact values.
The "100 requests change" column includes writing 100 files with `fsync`.

Reading them honestly: parsing, generation and disk writes are proportional to
the change, and an unrelated event costs a `stat`. But a batch still costs one
linear in-memory pass over all routes. In a separate attribution run at 20,000
routes, cloning the cached requests took about 15 ms and `reconcile::plan`
about 70 ms (roughly 3.5 µs per route). That is why one edited route costs
about 3, 9 and 68 ms as the project grows: roughly 3 to 11 times cheaper than a
full resync, but not proportional to the edit. Decision: **keep the single
authoritative reconciler**. Restricting it to affected routes would require
reasoning about uniqueness across the whole set (the area where the property
tests found four bugs in phase 5) to save time that is only noticeable far
above realistic NestJS sizes. If measurements in the hardening phase show it
matters, it can be added behind the same property test.

An attempted optimization (keeping the stamp in `GeneratedRequest` instead of
re-parsing it every batch) moved the one-route case at 20,000 routes from about
124 ms to about 105 ms in back-to-back runs, which is within the noise above.
It was kept because it removes redundant work and an impossible error path, not
because it is a demonstrated win.

Memory: state is replaced, not accumulated; a 1,500-edit session with changing
parse errors keeps all collections bounded (tested); standing diagnostics are
remembered up to 1,000 entries.

## Security

See [security.md](../security.md). In short: event paths are untrusted and
re-stat'ed; paths outside the project and symlinks are ignored; the collection
is never an input; sources are never written; deletion needs a complete view;
every destructive write re-verifies content; shutdown never leaves partial
files.

## Testing

- Unit: events and coalescer (deterministic, injected time), `Workspace`
  (modify, add, remove, remove directory, move directory in, rename, unrelated
  paths, broken file, collection edited meanwhile, file appearing meanwhile,
  generation cache reuse, routes-affected metric, 1,500-edit bounded memory,
  collection inside source), `process_batch` (registration order, overflow
  fallback, filtering, reporting), the stop flag.
- Properties: `is_relevant` agrees exactly with the directory walk (60 random
  trees); **any history of file writes, deletions, renames and touches,
  including broken sources, ends in the same collection as a fresh `sync`**
  (reference implemented independently of `Workspace`, 40 histories in the
  normal run).
- End-to-end (`tests/watch.rs`, real binary and OS watcher, Unix): initial
  sync, modify, new controller in a new directory (and a later file in it),
  removal, directory removal, editor-style atomic saves (one logical change),
  40 files at once, unrelated files and edits of the collection trigger nothing
  (sentinel technique), no self-triggering, broken source keeps the watcher
  alive and destroys nothing then recovers, user-edited request becomes a
  conflict, directory move without duplicates, configuration reload and invalid
  configuration, `source = "."` with an existing collection, SIGINT/SIGTERM/
  SIGHUP exit 0, no partial or temporary files after shutdown, missing source,
  sources untouched. No fixed sleeps: tests wait on output lines.
- Stress: the watch suite was run 60 times in a row without failure.
- Mutation checks (each guard removed, tests must fail): keeping routes of a
  broken file (1 failure), deleting while a file is unparsable (1), ignoring
  lost events (1), no retry after a stale index (2), forgetting files below a
  removed directory (1), debounce from the first event (3), watching the
  collection (1), treating the collection as a source (1).

### Defects found while building this phase

- **A race, found by a flaky test (about 12% of runs):** the configuration file
  was read immediately on its event, so a truncate-then-write could be read
  while empty (a valid default configuration), causing a spurious reload and
  two rewrites of every request. Fixed by debouncing configuration events.
- The collection directory was watched and treated as input when it lies inside
  the source tree (`source = "."`). Fixed and tested.
- `routes_affected` ignored parameter changes and counted unchanged routes.
- Problems from registering new directories were silently swallowed by a
  placeholder (caught in review before tests); now reported.
- A stale diagnostics set could grow without bound; capped.

## Limitations

- Edits made in Bruno while watching are not noticed until a write would touch
  them or a full recomputation happens (restart, config change, lost events).
- One linear in-memory pass per batch (see Performance).
- Deletion is suppressed globally while any source file is unparsable.
- Not run on macOS or Windows; the end-to-end tests are Unix-only. Network and
  container filesystems may deliver no events.
- The signal handler is process-wide (installed once per process).
- Directory symlinks are not watched (not followed), as in discovery.
- `source` must be a directory inside the project; a huge `source` is limited by
  the OS watch limit.

## Acceptance criteria

All items of the Phase 6 prompt are met within the limitations above; fmt,
clippy (`-D warnings`) and all 443 tests pass.

## Commits

See `git log`: `feat: add incremental watch mode`,
`test: add watch mode end-to-end tests`, `docs: document phase 6`.

## What later phases build on

Phase 7 (staged Git) can reuse `Workspace::refresh`'s structure, but the
analyzer currently reads files from disk (`nest::analyze_each`); it needs an
input abstraction (path plus in-memory content) so staged blobs can be parsed,
and a way to run the reconciler without a live watcher session.
