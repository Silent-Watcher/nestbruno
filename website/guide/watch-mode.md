# Watch Mode

`nestbruno watch` performs an initial sync, then keeps the collection
synchronized with your edits — re-parsing only the files that changed.

```txt
Initial sync:
  Created: 4  Updated: 0  Deleted: 0  Conflicts: 0  Unchanged: 0
Watching src (3 directories). Press Ctrl-C to stop.
Changed: src/users/users.controller.ts
  Routes affected: 1
  Created: 0  Updated: 1  Deleted: 0  Conflicts: 0
```

## What gets watched

One non-recursive watch per directory that discovery would enter — so
`.git`, `node_modules`, configured excludes, over-deep trees **and the
collection directory itself** are never watched (no feedback loops, even
with `source = "."`). `nestbruno.toml` is watched in the project root:
a valid change reloads the configuration, an invalid one is reported and
the old configuration stays active.

## Events are hints

Editors save files in weird ways (write temp, rename over, touch, bursts of
events). Raw events are normalized and debounced (default 200 ms,
`--debounce 10..60000`), but the result is only a set of **dirty paths**:
each is re-statted and re-read from disk, so event ordering and rename
choreography cannot misrepresent the state. Duplicates collapse; more than
10,000 paths degrade safely to a full resync.

## Incremental by construction

- A changed file is parsed once; only its controllers' requests are
  regenerated (a per-controller cache makes the untouched ones free).
- The same reconciliation rules as `sync` run over an in-memory view of the
  collection; only planned writes touch the disk.
- A file that currently does not parse **keeps its last good routes** and
  suppresses deletions — a half-saved controller destroys nothing, and the
  request reappears as soon as the file parses again.
- Renames and moves need no special case: both paths become dirty and the
  matching fingerprints update the requests in place.

## Fallbacks

Everything is recomputed from disk when correctness cannot be guaranteed:
lost events (channel full / overflow), watcher errors, or a failed
incremental step. A property test asserts that **any history of edits ends
in the same collection as a fresh `sync` of the final sources**.

## Shutdown

SIGINT, SIGTERM, SIGHUP (and Ctrl-C on Windows) finish the batch in
progress — all writes are atomic — print `Stopped.` and exit 0. A closed
stdout also stops the process (no broken-pipe noise in `piped` setups).

## Known limits

- Edits made in Bruno *while watching* are noticed only when a write would
  touch them (write-time verification) or at a full resync.
- A batch costs one linear in-memory pass over all routes (fine up to tens
  of thousands of routes; measured numbers in the repository docs).
- Network / container filesystems may deliver no events.
