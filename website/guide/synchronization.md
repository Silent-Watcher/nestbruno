# Synchronization & Ownership

`nestbruno sync` is built around one rule: **the source is the truth about
which routes exist, and everything you edited is yours.**

## The stamp

Every generated request ends its `docs` block with one machine line:

```txt
NestBruno: v1 id=<16 hex> key=<16 hex> anchor=<16 hex> hash=<16 hex>
```

- `hash` covers the generated content (minus the stamp and the `seq` line).
  A file counts as *modified by you* exactly when its content no longer
  hashes to this value. Reordering requests in Bruno (`seq`) is not an edit.
- The stamp is a **claim, never an authority** — the content is always
  re-verified against `hash` before any write.
- Delete the stamp line and the file becomes *unknown* → it is preserved.
  That is the safe failure mode.

The stamp lives in `docs` because Bruno drops unknown `meta` keys on
re-save but keeps `docs`.

## Matching a source route to an existing request

Three fingerprints of the same route (stable, nothing machine-specific):

| Fingerprint | Covers | Lets sync follow... |
| --- | --- | --- |
| `id` | controller (class + file), method, path, handler | exact match |
| `key` | controller class, method, path | a handler rename or a controller file move |
| `anchor` | controller class, handler | a path or HTTP-method edit |

Weaker fingerprints are used only when unique on both sides and not
contradicted; an ambiguous file matches nothing instead of guessing.

## What sync does

| Situation | Result |
| --- | --- |
| New route | `CREATE` (never overwrites an existing file) |
| Route changed, request unmodified | `UPDATE` in place (keeps your `seq`) |
| Route changed, request modified by you | `CONFLICT` — you decide |
| Route gone, request unmodified | `DELETE` |
| Route gone, request modified by you | `CONFLICT` — you decide |
| Hand-written request, environment, `folder.bru` | `PRESERVE` |
| Route exists but can't be generated (e.g. `@All`) | `PRESERVE` + warning |
| Two files claim one route (**critical**) | nothing is written at all |

Critical conflicts (duplicate identities — a common cause is Bruno's
"clone request", which copies the stamp) block the entire run: the plan is
shown, zero files are touched. The suggestion tells you which stamp line to
delete.

## Safety rules

1. The whole plan is computed before anything is written.
2. Replace/delete re-verify the file content immediately before the
   operation; a file that changed since planning is left alone and reported.
3. Nothing is deleted when any source file failed to analyze — "no route
   found" may mean "could not look".
4. All writes are atomic (temp file + fsync + rename); a failed write leaves
   the old file, a failed serialization produces no file.
5. Folders emptied by deletion are removed after all operations;
   `bruno.json` is never touched.

## Dry run

`sync --dry-run` runs the identical pipeline up to the plan and prints it —
nothing is written, not even the collection directory. The dry-run lists are
the same as the real run's (property-tested).

```txt
CREATE    bruno/Users/new.bru
UPDATE    bruno/Users/find-all.bru
DELETE    bruno/Users/old.bru
CONFLICT  bruno/Users/x.bru: generated file was modified
          what to do: <suggestion>
Created:     1
Updated:     1
Unchanged:   8
Deleted:     1
Preserved:   4
Conflicts:   1
No files were written.
```

`-v` also lists `PRESERVE` and `UNCHANGED` files.

## Verification

`nestbruno verify` answers a different question than `sync --dry-run`:
"is the collection in sync?" — and exits non-zero when it is not, which
makes it the right command for CI and Git hooks.

- It runs the identical pipeline as a dry run (same plan, same ownership
  rules) and never writes anything — not even on errors or conflicts.
- The report groups the plan into `missing` / `outdated` / `stale`
  requests, conflicts, and preserved files, with exit 0 (in sync),
  1 (out of sync — CI failure) or 4 (conflicts or an incomplete analysis).
- `verify --staged` verifies against the Git index state, like
  `sync --staged`.
- `verify --format json` gives the same information as one JSON document.

Use `--dry-run` when you want to see the plan (it always exits 0); use
`verify` when a process needs a pass/fail answer. Both are strictly
read-only.

## Resolving a conflict

A conflict means: the route changed **and** you edited the request. NestBruno
will not pick a winner. Your options:

- keep your version: do nothing — the conflict is reported on every sync
  until the file changes;
- accept the new shape: delete the `.bru` file (or just its `NestBruno:`
  stamp line to keep the file as "unknown") and run `sync` again.
