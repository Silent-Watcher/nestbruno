# Project status

## Implemented

- **Phase 1 (foundation):** CLI skeleton, configuration loading and
  validation, project root discovery, error model with exit codes,
  diagnostics reporter, test infrastructure, CI workflow.

- **Phase 2 (controller discovery):** deterministic discovery of controller
  candidate files from `source`/`include`/`exclude`, with directory pruning,
  symlink and depth protection, and a recoverable-vs-fatal error policy.

- **Phase 3 (route extraction):** static oxc-based analysis of `@Controller`
  classes and `@Get/@Post/@Put/@Patch/@Delete/@Head/@Options/@All` methods with
  `@Param/@Query/@Body/@Headers` parameters into a Bruno-independent route IR,
  with source locations, unresolved-value tracking and per-file diagnostics.

- **Phase 4 (Bruno generation):** deterministic `.bru` files (one folder per
  controller, one file per route), collision-safe naming, route identity,
  safe atomic writer with an explicit existing-file policy. Output verified
  against Bruno's own parser/serializer.

- **Phase 5 (synchronization):** ownership stamp, classification, pure
  reconciler, conflicts, verified atomic apply, `sync` and `sync --dry-run`.
  Property-tested: syncing a history of changes equals syncing the final state
  once.

- **Phase 6 (watch and incremental synchronization):** `watch` keeps the
  collection synchronized with the changed files only, with debounced
  filesystem events, per-controller caching, safe handling of broken files,
  configuration reload, clean shutdown and full-recompute fallbacks. Property
  test: any history of changes equals a fresh sync.

## Commands

| Command | State |
| --- | --- |
| `init` | Works: writes `nestbruno.toml`, never overwrites. |
| `check` | Works: validates configuration and that the source directory exists. |
| `discover` | Works: lists candidate controller files, one per line. Diagnostic/inspection command. |
| `routes` | Works: prints extracted routes, parameters and locations. Diagnostic/inspection command (text format, not a stable machine interface). |
| `generate [--force]` | Works: writes request files; existing differing files are left alone unless `--force`; nothing is ever deleted. |
| `sync [--dry-run]` | Works: creates, updates in place, deletes unmodified generated requests of removed routes; preserves unknown files; reports conflicts (exit 4). |
| `watch [--debounce MS]` | Works: initial sync, then incremental synchronization on change until Ctrl-C/SIGTERM. |

## Current limitations

- Decorators are matched **by name**; aliased imports, namespace imports
  (`common.Get`), custom decorators and array paths (`@Get(['a','b'])`) are
  not recognized or are reported as unresolved.
- Only top-level classes are analyzed; controller inheritance, global
  prefixes (`app.setGlobalPrefix`), versioning and `RouterModule` paths are
  not modelled, so extracted paths can differ from the running application's.
- Sources over 256 KiB are skipped (reported as errors).
- No field-level merge: any user edit locks the whole request (conflict if its
  route changes). `seq` is user-owned after creation.
- Nothing is deleted while source discovery/analysis is incomplete.
- `sync` still reads every existing `.bru` file per run; `watch` keeps an
  in-memory index instead, but does not watch the collection (edits made in
  Bruno meanwhile are caught by write-time verification).
- Watch mode costs one linear in-memory reconciliation pass per batch
  (roughly 10 to 20 ms at 5,000 routes and 70 to 100 ms at 20,000, from
  noisy sandbox runs); parsing and writes are
  proportional to the change.
- `@All`, unresolved/array paths and paths with `?`, `#`, whitespace or `{{`
  are not generated; bodies are skeletons; no environment or `folder.bru` is
  created (`baseUrl` must be defined in a Bruno environment).
- Output verified with Bruno's parser library, not by opening the desktop app.
- Discovery measurement so far: ad-hoc runs on synthetic trees (100,000-file
  discovery; 5,000-controller extraction, 5,000-file generation; 5,000-request sync; see the phase documents). No benchmark suite exists yet (planned with the hardening phase).
- CI is configured for Linux, macOS and Windows but has only been exercised
  locally on Linux.
- No `LICENSE` file has been chosen yet (owner decision).

## Known issues

None.
