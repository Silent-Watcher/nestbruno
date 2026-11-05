# Phase 5 — Synchronization & Reconciliation

**Ships:** `nestbruno sync [--dry-run]` — reconciliation that never destroys
user work, plus the ownership model the whole tool now stands on.

## The design in one sentence

The planner is a **pure function** from (desired requests, classified
existing files) to a plan; only after the full plan exists — and contains no
critical conflicts — does anything touch the disk, through hash-re-verified
atomic writes.

## Ownership

Every generated request ends its `docs` block with a stamp:
`NestBruno: v1 id=… key=… anchor=… hash=…`. The `hash` decides "modified by
user" — reordering in Bruno (`seq`) is not an edit. No stamp → unknown →
preserved. Invalid stamp → conflict. There is **no field-level merge**: a
file you edited is yours, and a route change on it is a conflict you
resolve.

## Three fingerprints, no false matches

`id` (exact), `key` (controller+method+path — follows handler renames and
file moves), `anchor` (controller+handler — follows path/method edits).
Weaker tiers apply only when unique on both sides and not contradicted;
ambiguity matches nothing rather than guessing.

## Deletion safety

Only positively-stamped, unmodified requests of routes that no longer exist
are deleted — and only when the analysis was **complete** (a half-saved
controller can never wipe requests). Folders are emptied after all
operations; failures are per-operation and exit 4.

## Dry run

`sync --dry-run` runs the identical pipeline up to the plan and prints it:
nothing is written, not even the collection directory. Property tests assert
the dry run predicts the real run and leaves the tree byte-identical.

## Found by property tests, kept as regressions

The property suites (any history of changes ≡ fresh sync of the final state;
user edits survive any later history) found real bugs during development —
e.g. routes that became unresolvable would have had requests deleted as
stale, and a folder was removed before a sibling creation could use it. All
are permanent regression tests now.

::: tip Full record
The complete reconciliation rule table, conflict taxonomy, safety rules and
mutation-check evidence: `docs/phases/phase-05.md` in the repository.
:::
