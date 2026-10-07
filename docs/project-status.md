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

## Commands

| Command | State |
| --- | --- |
| `init` | Works: writes `nestbruno.toml`, never overwrites. |
| `check` | Works: validates configuration and that the source directory exists. |
| `discover` | Works: lists candidate controller files, one per line. Diagnostic/inspection command. |
| `routes` | Works: prints extracted routes, parameters and locations. Diagnostic/inspection command (text format, not a stable machine interface). |
| `sync`, `watch` | Load the project, then fail with "not implemented". |

## Current limitations

- Decorators are matched **by name**; aliased imports, namespace imports
  (`common.Get`), custom decorators and array paths (`@Get(['a','b'])`) are
  not recognized or are reported as unresolved.
- Only top-level classes are analyzed; controller inheritance, global
  prefixes (`app.setGlobalPrefix`), versioning and `RouterModule` paths are
  not modelled, so extracted paths can differ from the running application's.
- Sources over 256 KiB are skipped (reported as errors).
- No Bruno functionality exists yet.
- Discovery measurement so far: ad-hoc runs on synthetic trees (100,000-file
  discovery; 5,000-controller extraction, see phase-02.md and phase-03.md). No benchmark suite exists yet (Phase 10).
- CI is configured for Linux, macOS and Windows but has only been exercised
  locally on Linux.
- No `LICENSE` file has been chosen yet (owner decision).

## Known issues

None.
