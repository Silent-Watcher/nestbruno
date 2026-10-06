# Project status

## Implemented

- **Phase 1 (foundation):** CLI skeleton, configuration loading and
  validation, project root discovery, error model with exit codes,
  diagnostics reporter, test infrastructure, CI workflow.

- **Phase 2 (controller discovery):** deterministic discovery of controller
  candidate files from `source`/`include`/`exclude`, with directory pruning,
  symlink and depth protection, and a recoverable-vs-fatal error policy.

## Commands

| Command | State |
| --- | --- |
| `init` | Works: writes `nestbruno.toml`, never overwrites. |
| `check` | Works: validates configuration and that the source directory exists. |
| `discover` | Works: lists candidate controller files, one per line. Diagnostic/inspection command. |
| `sync`, `watch` | Load the project, then fail with "not implemented". |

## Current limitations

- Discovery lists candidate *files*; it does not check that they contain a
  controller. Nothing parses TypeScript yet.
- No route extraction or Bruno functionality exists yet.
- Discovery measurement so far: one ad-hoc run on a synthetic 100,000-file
  tree (see phase-02.md). No benchmark suite exists yet (Phase 10).
- CI is configured for Linux, macOS and Windows but has only been exercised
  locally on Linux.
- No `LICENSE` file has been chosen yet (owner decision).

## Known issues

None.
