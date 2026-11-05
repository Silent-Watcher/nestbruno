# Phase 5: Bruno synchronization and reconciliation

## Objective

Keep an existing Bruno collection in step with the NestJS source **without
destroying user work**. The source is the truth about which routes exist;
everything the user changed in a request is theirs.

```text
route IR -> desired requests (bruno::generate, pure)
                          \
                           reconcile (pure) -> Plan -> apply -> SyncResult
                          /
existing .bru files (discovered and classified)
```

Command: `nestbruno sync [--dry-run]`. `generate [--force]` from Phase 4 is kept
as the blunt "write what is missing / force-overwrite" tool; it now writes the
same stamped files.

## Scope

Implemented: collection discovery, ownership stamp, classification,
reconciliation, conflicts, safe deletion, dry run, structured result, atomic
verified writes. Not implemented: watch, incremental analysis, Git, caching.

## Architecture

| Module | Role |
| --- | --- |
| `bruno/stamp.rs` | The ownership stamp and "normalized" content hashing |
| `bruno/identity.rs` | `id`, `key`, `anchor` fingerprints, `content_hash` |
| `sync/existing.rs` | Discover and classify existing `.bru` files (bounded, no symlinks) |
| `sync/reconcile.rs` | **Pure** `plan(desired, existing) -> Plan` |
| `sync/apply.rs` | Executes a plan through the safe primitives |
| `sync/mod.rs` | Types (`Plan`, `SyncResult`, `Conflict`, ...) and `synchronize()` |
| `bruno/writer.rs` | Still the only module that mutates the filesystem |

The sync layer consumes `GeneratedRequest`s only. It knows nothing about
TypeScript or decorators, prints nothing and does not depend on Git.
`SyncResult` is terminal-independent and is what later phases (watch, staged,
incremental) will consume.

## Route identity

Three fingerprints of the route (FNV-1a, stable, nothing machine specific):

| Name | Covers | Used for |
| --- | --- | --- |
| `id` | controller (class + file), method, path, handler | exact match |
| `key` | controller class, method, path | follows a handler rename or controller move |
| `anchor` | controller class, handler | follows a path or method edit |

`key` and `anchor` are only used when unique on both sides **and not
contradicted**: candidate pairs of both tiers are gathered together and a pair
is accepted only if neither side appears in another pair. A file plausibly
claimed by two different routes is ambiguous and matched to neither.

A route whose path/method makes it ungeneratable (`@All`, dynamic path) is
recorded as *skipped*; its existing request (matched by id, key or anchor) is
**preserved**, never treated as stale.

## Generated metadata (the stamp)

One line, last in the request's `docs` block:

```text
NestBruno: v1 id=<16 hex> key=<16 hex> anchor=<16 hex> hash=<16 hex>
```

`hash` is the hash of the file as generated with the stamp line **and the `seq`
line** removed. It is a claim to be verified, never an authority.

- Stored in `docs`, not `meta`: Bruno drops unknown `meta` keys on re-save but
  keeps `docs`. Verified with Bruno's own library: parse, change `seq`,
  serialize keeps the stamp byte-for-byte.
- If a user deletes the line the file becomes *unknown* and is preserved: the
  safe failure mode.
- `seq` is ignored when judging modification (reordering in Bruno is not an
  edit) and **is kept** when a file is updated. After creation `seq` is
  user-owned.

## Ownership classes

| Class | Recognized by | Treatment |
| --- | --- | --- |
| generated, unmodified | valid stamp, content hash matches | may be updated / deleted |
| generated, modified | valid stamp, content hash differs | never updated or deleted; conflict only if the route changed |
| unknown | no stamp (hand-written requests, `folder.bru`, environments, ...) | preserved |
| invalid | unreadable, binary, not UTF-8, > 4 MiB | preserved + diagnostic. For classification, unknown files keep their text in memory only up to 64 KiB |
| invalid metadata | a NestBruno line that cannot be parsed, or two of them | **conflict**; untouched |

"Owned fields": everything the generator writes is tool-owned *as long as the
file is unmodified*. Any other change (headers, scripts, auth, body, docs,
name, URL, tests) makes the whole file user-owned until it is deleted. There is
no field-level merge, by design: when merging is not provably safe, report a
conflict.

Phase 4 files (no stamp) are adopted only if byte-identical, apart from `seq`,
to what would be generated; anything else stays unknown.

## Reconciliation rules (`sync/reconcile.rs`)

| Situation | Result |
| --- | --- |
| New route, nothing at its path | `CREATE` |
| Matched, stamp already records the current representation | `UNCHANGED` (even if the user edited the file: nothing to merge) |
| Matched, route changed, file unmodified | `UPDATE` in place (location and `seq` kept) |
| Matched, route changed, file modified | `CONFLICT` (`GeneratedFileModified`) |
| Path held by a stale unmodified generated file | `UPDATE` (replace in place) |
| Path held by another route's generated file | `CREATE` under `<name>-<8 hex of id>.bru` |
| Path held by anything else | `CONFLICT` (`TargetOccupied`), nothing overwritten |
| Generated file, route gone, unmodified | `DELETE` (only if the run was complete) |
| Generated file, route gone, modified | `CONFLICT` (`StaleGeneratedFileModified`) |
| Route exists but could not be generated | `PRESERVE` + warning |
| Unknown / invalid file | `PRESERVE` |
| Two routes with one identity (**critical**) | `CONFLICT`; nothing is written |
| Two files claiming one identity (**critical**) | `CONFLICT`; nothing is written |

Every conflict is structured (`ConflictReason`: file, route id, description,
suggested action). Output order is deterministic (sorted by path).

## Safety rules

1. The whole plan is computed before anything is written.
2. Critical conflicts: **nothing at all is written** (the plan is still shown).
   A common cause is Bruno's "clone request", which copies the stamp; the
   suggestion says to delete the `NestBruno:` line of the copy.
3. Only positively generated, unmodified files are replaced or deleted, and
   their content is **re-verified by hash immediately before** the operation; a
   file that changed since planning is left alone and reported as failed.
4. **Nothing is deleted when discovery or analysis was incomplete** (a source
   file did not parse, a directory was unreadable): "no route found" may mean
   "could not look". This keeps a half-saved controller from wiping requests.
5. A request is created only where no file exists (no-clobber).
6. Folders emptied by deletions are removed **after** all operations (a bug
   found by the property tests, see below); `bruno.json` is never touched.
7. Failures are per operation; the others continue. Exit code 4 reports them.

## Dry run

`sync --dry-run` runs the same pipeline up to the plan and prints it; nothing
is written, not even the collection directory. Tests assert that the dry run
leaves the tree byte-identical and that its created/updated/deleted lists equal
those of the real run (also as a property).

Output (deterministic):

```text
CREATE    bruno/Users/new.bru
UPDATE    bruno/Users/find-all.bru
DELETE    bruno/Users/old.bru
CONFLICT  bruno/Users/x.bru: <reason>
          what to do: <suggestion>
FAILED    bruno/Users/y.bru: <reason>      (only when an operation failed)
Created:     1
Updated:     1
Unchanged:   8
Deleted:     1
Preserved:   4
Conflicts:   1
No files were written.            (dry run or blocked)
```

`-v` also lists `PRESERVE` and `UNCHANGED` files. Exit code: 0 when fully
synchronized (a dry run with pending changes is 0), 4 on conflicts, failures or
incomplete source/collection reads.

## Filesystem safety

Contained paths only (validated components, collection walked from the project
root), symlinks refused or never followed, no write through existing
symlinks/directories, atomic temp + `fsync` + rename/no-clobber link, bounded
reads, no shell, no code execution. Unsafe components in a hand-crafted plan
(`../x`, absolute paths, empty names) are rejected by the applier even though
the planner never produces them (tested).

## Bugs the tests found (permanent regression tests)

- Skipped routes were not recorded when `build_request` failed, so a route that
  became unresolvable would have had its request deleted as stale.
- A path becoming unresolvable changes the identity, so id/key could not
  protect the existing request; this led to the third fingerprint, `anchor`.
- Deleting the last file of a folder and creating a sibling in the same run
  failed (folder removed too early).
- One file claimed by two routes through different tiers produced a spurious
  conflict; now ambiguous claims match nothing.
- In-place updates keep old file names; a new route wanting a kept name got a
  conflict; now it gets a deterministic alternative name.
- A parse failure of a folder component was mapped to "the collection root"
  (caught in review, before any test): now an error.

The last four were found by property tests (3,000 cases per property run once;
48 in the normal suite).

## Performance

Reconciliation uses hash maps and is linear in requests + files; each existing
file is discovered and read once (bounded) and text is kept only for stamped
files and files at a target path. Unchanged files are never rewritten.

Measured (release, Linux sandbox, single runs, 5,000 requests in 50
controllers): no-op sync 0.10 s, dry run 0.10 s (also with 5,000 updates
pending: 0.11 s), first sync 2.6 s, 5,000 updates 3.6 s (atomic temp file +
`fsync` per file, as measured in Phase 4), deleting 4,400 requests 0.55 s,
peak memory about 11 MB. Observations, not a benchmark suite (Phase 10).

## Testing

Unit: stamp (format, malformed, ambiguity, seq, properties), existing-file
loading and classification, **reconciler** (every rule above, ordering,
fixed point), applier (hostile plans, stale content, symlinks). Integration
(`tests/sync.rs`, 28 tests, real binary): the cycle create / unchanged /
update / delete, every conflict kind, renames, moves, duplicates, dry run,
incomplete source, invalid and unknown files, legacy adoption, hostile source,
forged stamps, symlinks, a 3,000-request collection. Properties: incremental
history equals a fresh sync of the final state; a dry run never writes and
predicts the real run; user edits survive any later history.

**Mutation checks** (each guard removed, tests must fail): treating modified
files as unmodified (8 failures), deleting despite incomplete analysis (4),
skipping the pre-write re-check (1), ignoring critical conflicts (2), not
protecting ungeneratable routes (2), adopting any unknown file at a target
path (4). Verification outside the suite: files after a sync update still
round-trip through Bruno's parser/serializer.

## Limitations

- No field-level merge: any user edit locks the whole file (conflict when the
  route changes). A user who wants regeneration deletes the file.
- Renaming a path parameter *and* the handler together, or moving a controller
  and renaming its class, looks like delete + create.
- `seq` is not updated after creation.
- Deletion is suppressed globally (not per source file) when anything is
  incomplete.
- The hash is FNV-1a (64 bit): an accidental collision would hide a user edit
  with probability 2^-64; it is not collision-resistant against deliberate
  forgery, which only affects files inside the collection the forger can
  already edit.
- Check-then-write races remain (a re-verified hash narrows but does not close
  the window).
- `generate` and `sync` overlap; `generate` remains for forced regeneration.
- Windows untested.

## Acceptance criteria

All items of the Phase 5 prompt's sections 3-23 are met within the limitations
above; fmt, clippy (`-D warnings`) and all tests pass.

## Commits

See `git log`: `feat: add Bruno synchronization and reconciliation`,
`test: add synchronization tests and update golden fixtures`,
`docs: document phase 5 synchronization`.

## What later phases build on

Phase 6 reuses `reconcile::plan`, `apply` and `SyncResult` unchanged, feeding
them an in-memory view instead of re-reading the collection (see
[phase-06.md](phase-06.md)); a restricted-to-affected-routes variant was
considered and deliberately not built, because measurements did not call for
it. `Plan` and `SyncResult` remain the input for any further reporting.
