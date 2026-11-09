# Commands

Global flags (usable with every command):

| Flag | Meaning |
| --- | --- |
| `-C, --project <DIR>` | use this directory as the project root instead of searching upwards |
| `-v, --verbose` | also print `note:`-level diagnostics |

## JSON output

`discover`, `routes` and `sync` accept `--format json` and print **exactly
one JSON document** to stdout, suitable for scripts, CI and AI agents:

```bash
nestbruno routes --format json
nestbruno discover --format json
nestbruno sync --dry-run --format json
nestbruno sync --staged --format json
```

What the documents contain:

- `routes`: every route with `controller`, `handler`, `method`, `path`,
  `parameters` (kind, name, index, type annotation, resolved DTO fields),
  the `body` shape and the `source` location, plus `diagnostics` and a
  `complete` flag that is `false` when some path could not be processed.
- `discover`: the candidate `files` (deterministically sorted),
  `directories_read`, `diagnostics`, `complete`.
- `sync`: `dry_run`, `applied`, `complete`, `staged_changes`, and the
  result lists `created` / `updated` / `unchanged` / `deleted` /
  `preserved`, `conflicts` (with a stable machine-readable `reason`,
  a `description` and a `suggestion`), `failed` operations and
  `diagnostics`.

Rules:

- diagnostics are structured objects *in* the document; human-readable
  diagnostics and progress lines keep going to stderr, so stdout stays
  parseable;
- human-readable text is the default and is unchanged;
- exit codes are unchanged (0 success, 3 configuration error,
  4 incomplete, 1 runtime failure);
- output is deterministic: repeated runs on identical input are
  byte-identical.

## `nestbruno init`

Writes a `nestbruno.toml` with the documented defaults. Never overwrites an
existing file (exit 1 if one exists).

## `nestbruno check`

Validates the configuration and that the source directory exists. Prints the
effective settings. Use this to verify a project layout.

## `nestbruno discover`

Lists candidate controller files, one per line (deterministic, sorted).
Candidates come from paths alone — a file here may still contain no
controller. Exits 4 if some directories could not be read.

## `nestbruno routes`

Prints the routes found by analysis, e.g.

```txt
GET /users/:id  UsersController.findOne  src/users/users.controller.ts:12:3
  @Param(id) #0
```

This is a diagnostic text format for humans, not a stable machine interface.

## `nestbruno generate [--force]`

The blunt tool: writes a request file for every generatable route.

- existing identical files are not rewritten;
- existing **different** files are left untouched with a warning, unless
  `--force` replaces them atomically;
- nothing is ever deleted, `bruno.json` is never replaced.

Prefer `sync` — `generate` exists for forcing a clean rewrite of
unmodified-generated content and for bootstrapping.

## `nestbruno sync [--dry-run]`

The safe tool: reconcile the collection with the routes found in source.
Plans everything first, then creates, updates in place, and deletes only
unmodified generated files of routes that no longer exist. Your edited
requests become conflicts and are never overwritten; unknown files are
preserved; nothing is deleted when any source file failed to analyze.

With `--dry-run` the same plan is printed and **nothing** is written — not
even the collection directory. Exit code 0 even when changes are pending.

### `nestbruno sync --staged [--dry-run]`

Synchronize against the **Git index** instead of the working tree: the
routes analyzed are those of the staged content, exactly what is about to be
committed. Behavior:

- a staged addition/modification updates requests from the staged content;
- a staged deletion reconciles its requests;
- a staged rename updates requests in place;
- imported DTOs are resolved from staged content as well, never from the
  working tree; a staged DTO edit alone (with no staged controller) still
  updates the requests of its importers;
- if nothing relevant is staged, it is a clean no-op;
- Git is never modified, and the working-tree version of a staged file is
  never read.

## `nestbruno verify [--staged]`

Checks whether the generated collection still matches the sources — without
writing anything. It runs the same pipeline as `sync` (analysis,
generation, reconciliation) as a dry run and reports the differences:

```txt
NestBruno Verification

Status: OUT OF SYNC

Missing:   2
Outdated:  1
Stale:     1
Unchanged: 4
Conflicts: 0
Preserved: 3

Verification failed.
Run `nestbruno sync` to reconcile the collection.
The collection was not modified.
```

- **Strictly read-only:** no collection directory is created, no `.bru`
  file is created, updated or deleted, no ownership stamp or timestamp is
  touched — even on errors, conflicts and an incomplete analysis.
- Statuses: `IN SYNC`, `OUT OF SYNC` (missing/outdated/stale requests),
  `BLOCKED` (conflicts or failed operations), `INCOMPLETE` (the analysis
  could not establish a complete state), `NO STAGED CHANGES` (with
  `--staged`).
- Exit codes: **0** in sync, **1** out of sync (usable as a CI failure
  signal), **4** blocked or incomplete, plus the usual 2/3 for usage and
  configuration errors.
- `verify --staged` compares the collection against the Git index state
  (same semantics as `sync --staged`); with no relevant staged changes it
  is a clean no-op.
- `verify --format json` prints one JSON document: `status`
  (`in_sync`/`out_of_sync`/`blocked`/`incomplete`/`no_staged_changes`),
  `complete`, a `summary` with the category counts, `changes` (created/
  updated/deleted paths), `conflicts`, `failed` and `diagnostics`.

`verify` vs `sync --dry-run`: a dry run answers "what would sync do?" with
sync's exit code (0 even when changes are pending). `verify` answers "is
the collection in sync?" and fails (exit 1) when it is not — use it in CI;
use `--dry-run` when you want the plan.

## `nestbruno completions <shell>`

Prints a shell completion script for **Bash**, **Zsh**, **Fish** or
**PowerShell** to stdout, generated from the actual command tree:

```bash
nestbruno completions bash > ~/.local/share/bash-completion/completions/nestbruno
nestbruno completions zsh > "${fpath[1]}/_nestbruno"
nestbruno completions fish > ~/.config/fish/completions/nestbruno.fish
nestbruno completions powershell > nestbruno.ps1
```

- Nothing is written to disk and no shell configuration is changed: the
  command only prints. Redirect the output yourself (see above).
- Installing the script enables completions on the next shell start; until
  you place it where your shell looks, nothing changes. For Zsh, the
  target directory must be in your `fpath` before `compinit` runs.
- Generation is deterministic and needs no shell process: the script can
  be generated at package build time or redirected anywhere.

## `nestbruno watch [--debounce <MS>]`

Runs an initial sync, then keeps the collection synchronized while you edit.
Only changed files are re-parsed; only affected requests are reconciled.
Debounce default 200 ms (range 10–60,000). Stops cleanly on Ctrl-C /
SIGINT / SIGTERM / SIGHUP (exit 0) or when stdout closes. Details:
[Watch Mode](/guide/watch-mode).

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success (a dry run with pending changes is also 0; `verify` passes) |
| 1 | runtime failure (I/O, missing project directory, ...); `verify` also uses 1 when the collection is **out of sync** (CI failure signal) |
| 2 | usage error (clap) |
| 3 | configuration or project layout error |
| 4 | finished but **incomplete**: some source files or collection files could not be processed — see the `error:` diagnostics on stderr. Destructive sync actions are suppressed in this state. `verify` also uses 4 for conflicts (blocked) and an incomplete analysis. |
