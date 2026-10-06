# Project status

## Implemented

- **Phase 1 (foundation):** CLI skeleton, configuration loading and
  validation, project root discovery, error model with exit codes,
  diagnostics reporter, test infrastructure, CI workflow.

## Commands

| Command | State |
| --- | --- |
| `init` | Works: writes `nestbruno.toml`, never overwrites. |
| `check` | Works: validates configuration and that the source directory exists. |
| `sync`, `watch` | Load the project, then fail with "not implemented". |

## Current limitations

- No route extraction or Bruno functionality exists yet.
- CI is configured for Linux, macOS and Windows but has only been exercised
  locally on Linux.
- No `LICENSE` file has been chosen yet (owner decision).

## Known issues

None.
