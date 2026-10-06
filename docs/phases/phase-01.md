# Phase 1: Project foundation

## Objective

Establish the Rust CLI skeleton, configuration model, error handling,
diagnostics, test infrastructure, quality gates and documentation structure.
No NestJS or Bruno functionality.

## Scope

Cargo project; `init`, `check`, `sync` (stub), `watch` (stub); `nestbruno.toml`;
project root discovery; error and diagnostic model; CI workflow.

## Implementation

See [architecture.md](../architecture.md) for module responsibilities.

## Design decisions

- **Library + thin binary.** All logic is in the library for direct testing.
- **Config:** TOML, all keys optional, `deny_unknown_fields`. Keys: `source`,
  `bruno`, `include`, `exclude`. `source`/`bruno` are validated as relative
  paths without `..`. `bruno` cannot be the project root. `include` cannot be
  empty. Glob patterns are compiled in Phase 2.
- **Root discovery:** `--project DIR` is used verbatim. Otherwise walk up from
  the current directory for `nestbruno.toml`, then for `package.json`.
- **Errors:** one `thiserror` enum for fatal errors; recoverable per-path
  problems are `Diagnostic`s (three severities).
- **No logging framework.** A `Reporter` writing to stderr covers the need; a
  logging dependency would solve no current problem.
- **`check` command:** not in the original interface sketch. It exists so that
  configuration loading is observable end to end (and is useful to users).
- **Output:** results go to a `Write` (stdout), diagnostics to stderr, so a
  closed pipe exits quietly instead of panicking.

## Dependencies

| Crate | Problem solved | Why not std / existing |
| --- | --- | --- |
| `clap` (derive) | Argument parsing, help, usage errors | std has no parser; hand-rolling is error-prone |
| `serde` + `toml` | Typed config parsing with good error messages | std has no TOML |
| `thiserror` | Error enum boilerplate | Derive-only; avoids hand-written `Display`/`From` |
| `tempfile` (dev) | Isolated temp dirs for tests | std lacks auto-cleaned temp dirs |

All are mature, widely used and actively maintained.

## Tests

Unit tests: config parsing/validation (defaults, overrides, unknown keys,
traversal, absolute paths, size cap), root discovery precedence, error exit
codes, diagnostic formatting, CLI definition. Integration tests (`tests/cli.rs`)
run the real binary against temporary projects.

## Limitations

`sync` and `watch` are stubs. Include/exclude patterns are not yet compiled.

## Acceptance criteria

CLI starts; config loads; clean command structure; tests, fmt and clippy pass.

## Commits

See `git log` (`chore:`/`feat:`/`test:`/`docs:` prefixed).

## What future phases add

Phase 2 consumes `Project` and `Config` for controller discovery.
