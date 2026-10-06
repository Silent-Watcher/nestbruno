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
| Shell injection | No command is ever built from project data. No subprocesses exist yet. | by construction |

## Planned (not yet implemented)

Documented here so they are designed in, not forgotten:

- Symlink handling during traversal (Phase 2).
- Parser safety and resource limits (Phase 3).
- Output-path containment, atomic writes, overwrite protection (Phases 4-5).
- Safe Git invocation without shell interpolation (Phase 8).
- Bounded watcher event queues (Phase 7).

## Dependency security

Dependencies are kept minimal (see `Cargo.toml` comments for the reason each
exists). Run `cargo audit` locally if you have it installed; it is not yet part
of CI.
