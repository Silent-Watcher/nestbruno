# Security

The NestJS repository is **untrusted input**. This document lists the threats,
the mitigations that exist today, and the ones that are still planned. It does
not claim protections that are not implemented and tested.

## Implemented

| Threat | Mitigation | Tested |
| --- | --- | --- |
| Path traversal via configuration | `source` and `bruno` must be non-empty, relative, and free of `..`, root and drive-prefix components (`config.rs`). | unit + CLI tests |
| Writing into the project root | `bruno` must name a subdirectory. | unit test |
| Oversized configuration file | Reads are capped at 1 MiB. | unit test |
| Config typos silently ignored | `deny_unknown_fields`. | unit test |
| Overwriting an existing file in `init` | `create_new` (atomic exclusive create). | CLI test |
| Memory-unsafe code | `unsafe_code = "forbid"` crate-wide. | compiler |
| Symlinks inside the source tree | Never followed (`DirEntry::file_type` does not resolve links); reported as notes. Loops therefore terminate. | integration tests (unix) |
| Source directory symlinked outside the project | The resolved source root must lie inside the canonical project root, otherwise a fatal error. | integration test (unix) |
| Pathologically deep trees | Iterative walk (no recursion) with a depth limit of 64; deeper directories are reported, not entered. | integration test |
| Scanning dependency / VCS directories | `.git` and `node_modules` are never entered, regardless of configuration. | integration tests |
| Discovery writes or executes anything | It only lists directories and inspects file types; it never reads file contents. | by construction |
| Shell injection | No command is ever built from project data. No subprocesses exist yet. | by construction |

## Planned (not yet implemented)

Documented here so they are designed in, not forgotten:

- Parser safety and resource limits (Phase 3).
- Output-path containment, atomic writes, overwrite protection (Phases 4-5).
- Safe Git invocation without shell interpolation (Phase 8).
- Bounded watcher event queues (Phase 7).

## Known gaps

- Time-of-check/time-of-use races: the filesystem can change between
  `read_dir` and later phases reading a file. Later phases that read file
  contents must open files and handle failure; discovery results are a
  snapshot, not a lock.
- Windows junctions and reparse points are only handled to the extent that
  `std` reports them as symlinks; this is untested.

## Dependency security

Dependencies are kept minimal (see `Cargo.toml` comments for the reason each
exists). Run `cargo audit` locally if you have it installed; it is not yet part
of CI.
