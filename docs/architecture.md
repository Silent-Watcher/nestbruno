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
| `route.rs` | The route IR (`Controller`, `Route`, `HttpMethod`, ...) and `compose_path`. Independent of Bruno and of the parser. |
| `nest/extract.rs` | Pure function `extract_controllers(file, source)`: oxc AST in, IR + diagnostics out. The only place that parses TypeScript. |
| `nest/mod.rs` | `analyze_files`: bounded file reads, big-stack worker thread, per-file panic guard, aggregation. |
| `bruno/mod.rs` | `generate(config, controllers)`: pure planner from route IR to desired files (`GeneratedRequest`). No filesystem, no parsing. |
| `bruno/model.rs` | Bruno request model and IR-to-model mapping; validates all untrusted text. |
| `bruno/serialize.rs` | Model to `.bru` text; refuses anything Bruno could misread. |
| `bruno/naming.rs`, `bruno/identity.rs` | Deterministic safe names with collision handling; route identity and fingerprint. |
| `bruno/stamp.rs` | The ownership stamp (one `docs` line: id, key, anchor, content hash) and normalized hashing. |
| `sync/existing.rs` | Discovers and classifies existing `.bru` files (generated / modified / unknown / invalid). |
| `sync/reconcile.rs` | Pure `plan(desired, existing)`: matching, conflicts, deletions. No filesystem. |
| `sync/apply.rs`, `sync/mod.rs` | Execute a plan through verified writes; `SyncResult` (terminal independent); `synchronize()`. |
| `workspace.rs` | Incremental state for long-running modes: per-file analysis, per-controller generation cache, in-memory index of the collection; `refresh(dirty paths)` and `resync()`. |
| `watch/events.rs`, `watch/mod.rs` | Event normalization and debounce (pure); the watch loop, per-directory watches, shutdown. |
| `staged.rs` | The Git boundary for `sync --staged`: index snapshot, staged-change gate, staged content of every relevant file as in-memory sources. The only module that touches libgit2. |
| `bruno/writer.rs` | The only code that writes: containment, symlink refusal, overwrite policy, atomic files. |
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

## Route extraction

```text
Discovery.files ─▶ analyze_files (worker thread, large stack)
                     per file: symlink_metadata ─▶ bounded read ─▶ UTF-8
                               ─▶ oxc parse ─▶ semantic walk ─▶ Controller IR
```

- One read and one parse per file per run; the AST (arena allocated) is dropped
  when the file's IR is built. Nothing is cached by one-shot commands; watch mode keeps per-file state (see Watch mode).
- Syntax error ⇒ **no** routes from that file and one `error` diagnostic with
  `file:line:col`. A recovered AST is never used, so routes are never silently
  dropped from a half-parsed file.
- IR ordering: discovery order (sorted paths), then source order of classes,
  methods and parameters.
- Unresolvable decorator arguments become `RoutePath::Unresolved` /
  `ParameterName::Unresolved` plus a warning; nothing is evaluated or guessed.
- Recognition is name based and conservative; see the module docs of
  `nest/extract.rs` and [phases/phase-03.md](phases/phase-03.md).

## Bruno generation

```text
Analysis.controllers ─▶ generate()  ─▶ Vec<GeneratedRequest>   (pure, deterministic)
                                            │
                        write_collection() ◀┘  (safe atomic writes, overwrite policy)
```

The parser knows nothing about Bruno and the generator never reads source or
existing `.bru` files; the route IR is the only boundary. Computing the desired
state is separate from writing it, which is what reconciliation (Phase 5) and
dry-run need. Details, format verification and policies:
[phases/phase-04.md](phases/phase-04.md).

## Synchronization

```text
generate() ─▶ desired requests ─┐
                                ├─▶ reconcile::plan (pure) ─▶ Plan ─▶ apply ─▶ SyncResult
existing::load ─▶ classified ───┘                              (skipped on dry run / critical conflict)
```

Ownership comes from a stamp in each generated request; a file counts as
modified when its content no longer hashes to the stamp. Rules, conflicts and
safety guarantees: [phases/phase-05.md](phases/phase-05.md).

## Watch mode

```text
notify callback ─▶ bounded channel ─▶ loop ─▶ Coalescer (debounce) ─▶ dirty paths
                                                                          │
   Workspace::refresh: re-stat + re-read + re-parse ONLY those files ◀────┘
        │ replace their routes · regenerate only affected controllers (cached)
        ▼
   reconcile::plan (same pure rules as `sync`, over in-memory state)
        ▼
   apply only the planned changes ─▶ Delta ─▶ Notice ─▶ terminal
```

Events are hints: a path is merely dirty, and processing looks at the file as
it is now, so event order and rename sequences cannot misrepresent the state.
Lost events, watcher errors and failed incremental steps fall back to a full
recomputation from disk. A property test asserts that any history of source
changes yields the same collection as a fresh `sync` of the final state.
Details, measurements and limits: [phases/phase-06.md](phases/phase-06.md).

## Staged synchronization

```text
git index (+ HEAD) ─▶ staged.rs: staged-change gate ─▶ staged content of every
                     (unrelated files filtered by       relevant index entry
                      the discovery Matcher)                │
                                                            ▼
              analyze_sources (in-memory, same worker) ─▶ generate ─▶ synchronize
```

The index is the source of truth: staged content is parsed, never the
working-tree version, and the desired state is reconstructed from the whole
index (not patched onto a working-tree analysis). With no relevant staged
change the command is a clean no-op. Rules, security and limits:
[phases/phase-07.md](phases/phase-07.md).

## Planned components (not implemented)

None. See [roadmap.md](roadmap.md) for what comes after Phase 7.
