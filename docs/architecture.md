# Architecture

This document describes the system as it exists today. Sections for future
components are marked *planned* and are not implemented.

## Module responsibilities

| Module | Responsibility |
| --- | --- |
| `main.rs` | Thin binary: parse arguments, run, map errors to exit codes. |
| `cli.rs` | `clap` definitions only. No behavior. |
| `commands.rs` | One function per command. Writes results to a `Write`, diagnostics to a `Reporter`. |
| `config.rs` | `nestbruno.toml` model, parsing and validation. |
| `project.rs` | Project root discovery and the loaded `Project` (root + config). |
| `discovery.rs` | Finds controller *candidate* files from configuration and paths. Reads no file contents, knows nothing about Bruno, Git or the CLI. |
| `diag.rs` | `Diagnostic`, `Severity`, and the stderr `Reporter`. |
| `error.rs` | The fatal `Error` enum and exit-code mapping. |

All behavior lives in the library crate; `main.rs` only wires it up so every
code path can be tested without spawning a process.

## Data flow

```text
argv ──clap──▶ Cli ──▶ commands::execute
                          │
                          ├─ Project::find_root / Project::open ─▶ Config::load
                          ▼
                    stdout (results)   stderr (diagnostics)
```

## Error model

- **Fatal errors** (`Error`): invalid configuration, missing project, I/O
  failure that makes the command meaningless. They abort the command and map
  to an exit code.
- **Recoverable problems** (`Diagnostic`): confined to one path. Processing
  continues and the user is told. Severity: `Note` (verbose only), `Warning`
  (result complete, something unsupported), `Error` (result *incomplete* for
  that path, so later phases must treat that path's state as unknown).

Exit codes: `0` success, `1` runtime failure, `2` usage error (clap),
`3` configuration/project error, `4` finished but incomplete (some paths could
not be processed; see the `error:` diagnostics).

## Controller discovery

```text
Project ─▶ compile include/exclude globs (once)
        ─▶ resolve source root, verify it is inside the project
        ─▶ iterative directory walk
              dir  : skip .git/node_modules, skip excluded dirs, depth limit
              file : `.ts` (not `.d.ts`) ─▶ include && !exclude
        ─▶ sort ─▶ Discovery { files, diagnostics, directories_read }
```

`discover_controllers(&Project) -> Result<Discovery, Error>` is the whole API.
Discovery answers "which files *may* hold controllers" from paths alone
(Option A in the phase 2 prompt). Verifying that a candidate really contains
`@Controller` is the job of AST analysis, the only component that parses
TypeScript. Full semantics (precedence, symlinks, error policy) are in the
module documentation of `discovery.rs` and in
[phases/phase-02.md](phases/phase-02.md).

## Planned components (not implemented)

AST route extraction, Bruno generation, reconciliation,
incremental synchronization, watch mode, Git integration. See
[roadmap.md](roadmap.md).
