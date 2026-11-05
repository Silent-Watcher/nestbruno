# Phase 6 — Watch Mode

**Ships:** `nestbruno watch [--debounce MS]` — live, incremental
synchronization while you code.

## Principles

- **Events are hints.** Raw filesystem events are normalized, debounced
  (default 200 ms; a steady stream cannot starve processing) and collapsed
  into a set of dirty paths. Each is re-statted and re-read, so editors'
  write-temp-rename-over choreography cannot misrepresent the state.
- **Nothing is watched that matters not.** One non-recursive watch per
  directory discovery would enter; `.git`, `node_modules`, excludes and —
  importantly — the collection itself are never watched. No feedback loops,
  even with `source = "."`.
- **One authoritative rule set.** Watch runs the same pure reconciler as
  `sync`, over an in-memory view; only planned writes touch the disk. A
  property test asserts any history of edits ends in the same collection as
  a fresh `sync` of the final sources.

## Incremental behavior

- Only changed files are re-parsed (one parse each); only controllers whose
  source changed are regenerated (per-controller cache).
- A file that temporarily doesn't parse **keeps its last good routes** and
  suppresses deletions — a half-saved controller destroys nothing, and
  everything recovers when the file parses again.
- A request edited in Bruno meanwhile becomes a conflict, reported once —
  never overwritten (write-time hash verification plus one retry after
  re-reading).
- Renames and moves update requests in place via the Phase 5 fingerprints.

## Robustness

Lost events, watcher errors and failed incremental steps fall back to a
full recomputation from disk. A valid config change reloads the session; an
invalid one is reported and the old config stays active. SIGINT/SIGTERM/
SIGHUP finish the current batch, print `Stopped.` and exit 0 — no partial
files, ever (all writes are atomic). A closed stdout stops cleanly too.

## Measured

At 5,000 routes: a one-route edit costs ~9 ms end-to-end; an unrelated file
event ~0.1 ms (one `stat`); a full resync ~100 ms. One linear in-memory
pass over all routes remains per batch — measured, considered, and
deliberately kept for correctness and simplicity at realistic sizes.

::: tip Full record
Watcher architecture, the coalescer, the performance table, defects found
during the phase and their fixes: `docs/phases/phase-06.md` in the
repository.
:::
