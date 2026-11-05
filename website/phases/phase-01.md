# Phase 1 — Project Foundation

**Ships:** the `nestbruno` CLI skeleton, the `nestbruno.toml` configuration
model, project-root discovery, the error and diagnostics model, test
infrastructure and CI. Commands `init` and `check` work; `sync`/`watch`
exist as explicit stubs at this point.

## What it established

- **Library + thin binary.** All behavior lives in the library crate so
  every path is testable without spawning a process.
- **One config file, zero required keys.** `nestbruno.toml` with optional
  keys; unknown keys are rejected so typos are errors, not silent defaults.
- **Root discovery:** `--project DIR` wins, otherwise walking up finds the
  nearest directory with `nestbruno.toml`, then one with `package.json`.
- **Two-channel output:** results to stdout, diagnostics to stderr with
  severities `note < warning < error`; closed pipes exit quietly.
- **Exit codes** 0 / 1 / 2 (usage) / 3 (configuration) / 4 (incomplete),
  with "incomplete" meaning *some path's state is unknown* — a contract
  every later phase honors before deleting anything.

## Tests

Config parsing and validation (defaults, overrides, unknown keys, traversal,
absolute paths, size cap), root discovery precedence, exit-code mapping,
diagnostic formatting, plus CLI integration tests running the real binary
against temporary projects.

::: tip Full record
Design decisions, dependency rationale and limitations:
`docs/phases/phase-01.md` in the repository.
:::
