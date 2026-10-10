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
| Executing project code | TypeScript is only parsed to an AST and inspected structurally. No `eval`, no Node.js, no subprocess, no network. Decorator arguments are never evaluated. | by construction; tests assert dynamic expressions are not stringified |
| Parser stack overflow (a stack overflow *aborts the process*) | Measured on the pinned parser: up to 1394 B of stack per input byte in debug builds on deeply nested input. Sources are capped at 256 KiB and parsed on a worker thread whose stack is 2048 B per cap byte (~512 MiB, lazily committed virtual memory). | tests feed worst-case nesting at the cap through the real path; mutation-checked (shrinking the stack makes the test abort) |
| Panic inside the parser | Caught per file (`catch_unwind`); that file gets an `error` diagnostic, the run continues. | unit test of the guard (parser panics could not be provoked) |
| Oversized / non-UTF-8 / non-regular source files | Rejected with a diagnostic; reads are bounded to the cap + 1 byte; `symlink_metadata` rejects symlinks and special files before opening. | integration tests |
| Source text injected into `.bru` files (newlines opening `script:` blocks, `{{process.env.X}}` interpolation, `'''` values) | Control characters, Unicode line separators and `{{` rejected in the model; serializer re-validates and emits nothing on failure; body lines indented; JSON keys escaped. | unit + integration tests; mutation-checked; output parsed with Bruno's real parser |
| Path traversal / unsafe names from class, method or route text | Names reduced to an ASCII alphabet; route text never used in paths; writer re-validates every component (`..`, separators, `:`, control chars, reserved names). | unit + integration tests |
| Writing through symlinks / outside the collection | Collection path walked from the project root, symlinks refused at every level; existing symlink/dir targets never replaced. | unit + integration tests (unix) |
| Unsafe overwrite of user work | Existing differing files untouched unless `--force`; `bruno.json` never overwritten; non-generated files never touched. | unit + integration tests |
| Corrupt files after a crash | Temp file + `fsync` + no-clobber link / rename; no text on serialization failure. | unit tests |
| Destroying user edits during sync | A file is replaced or deleted only if positively generated **and unmodified** (content hash vs stamp); modified files are never touched; no field-level merge. | unit + integration + property tests; 6 mutation checks |
| Deleting on a partial view | No deletion when discovery, analysis or the collection read was incomplete; ungeneratable routes keep their requests. | unit + integration tests; mutation-checked |
| Acting on a file that changed after planning | Content hash re-verified immediately before replace/delete. | unit tests; mutation-checked |
| Ambiguous ownership (cloned requests, duplicate routes) | Critical conflict: nothing is written. | unit + integration tests |
| Forged or malformed stamps | A stamp is only a claim: modification is judged by the hash; malformed or duplicated stamps are conflicts; foreign files are never matched. | unit + property + integration tests |
| Discovery writes or executes anything | It only lists directories and inspects file types; it never reads file contents. | by construction |
| Shell injection | No command is ever built from project data. No subprocesses exist yet. | by construction |

## Planned (not yet implemented)

Documented here so they are designed in, not forgotten:

- Safe Git invocation without shell interpolation (Phase 8).
- Bounded watcher event queues (Phase 7).

## Known gaps

- The pre-write hash re-check narrows but does not close the window between check and write. The stamp hash is FNV-1a 64: not collision resistant against deliberate forgery by someone who can already edit the collection.
- Check-then-write races on the collection directory: another process could
  swap a directory for a symlink between the check and the write. The parent
  directory is not `fsync`ed.

- The stack bound is **empirical** for the pinned oxc version (see above). A
  parser upgrade can change per-level stack use; the worst-case-nesting test
  exists to fail loudly in that case, so re-run it on every upgrade.
- Reading a file is still racy: between the `symlink_metadata` check and
  `open`, a file could be swapped for a symlink. The consequence is limited to
  parsing the target's text (nothing is written or executed); `O_NOFOLLOW`
  would need platform-specific code and was not added.
- A 512 MiB virtual stack reservation can fail under strict memory limits
  (`ulimit -v`, strict overcommit); the run then fails with a clear error
  instead of analyzing without protection.

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
