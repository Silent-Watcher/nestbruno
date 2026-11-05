# Commands

Global flags (usable with every command):

| Flag | Meaning |
| --- | --- |
| `-C, --project <DIR>` | use this directory as the project root instead of searching upwards |
| `-v, --verbose` | also print `note:`-level diagnostics |

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
- if nothing relevant is staged, it is a clean no-op;
- Git is never modified, and the working-tree version of a staged file is
  never read.

## `nestbruno watch [--debounce <MS>]`

Runs an initial sync, then keeps the collection synchronized while you edit.
Only changed files are re-parsed; only affected requests are reconciled.
Debounce default 200 ms (range 10–60,000). Stops cleanly on Ctrl-C /
SIGINT / SIGTERM / SIGHUP (exit 0) or when stdout closes. Details:
[Watch Mode](/guide/watch-mode).

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success (a dry run with pending changes is also 0) |
| 1 | runtime failure (I/O, missing project directory, ...) |
| 2 | usage error (clap) |
| 3 | configuration or project layout error |
| 4 | finished but **incomplete**: some source files or collection files could not be processed — see the `error:` diagnostics on stderr. Destructive sync actions are suppressed in this state. |
