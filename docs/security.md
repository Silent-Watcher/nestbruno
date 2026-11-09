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
| Shell injection | No command is ever built from project data. No subprocesses exist (Git is accessed through the libgit2 C API, not a `git` process). | by construction |
| Malicious Git index/HEAD content (staged sync) | Index paths must be plain relative components; non-UTF-8 paths ignored; entries outside the project/source filtered before any read; blobs size-capped and UTF-8-checked like files; libgit2 runs no hooks and never spawns processes; the index is only read, never written. | unit + integration tests |
| Git state changing mid-run (staged sync) | The index is snapshotted once; later changes belong to the next run. Conflicted (unmerged) paths are excluded and make the run incomplete — nothing is deleted. | unit + integration tests |

## Planned (not yet implemented)

Documented here so they are designed in, not forgotten:

- Automated `cargo audit` in CI (Phase 8).

## Watch mode

| Threat | Mitigation | Tested |
| --- | --- | --- |
| Unbounded event queues / memory | Bounded channel; overflow flag; coalescer degrades past 10,000 paths to a full recompute; remembered diagnostics capped | unit tests; 1,500-edit session test |
| Feedback loops (our own writes re-triggering sync) | The collection directory is never watched and never a source, even when inside the source tree; nothing in the sources is ever written | end-to-end tests (`source = "."` too); mutation-checked |
| Watching dependency or VCS trees | Watches are per directory and follow discovery's pruning (`.git`, `node_modules`, excludes, depth) | end-to-end test counting watched directories |
| Untrusted event paths | Paths outside the project are ignored; symlinks and special files are never followed; every path is re-stat'ed before use | unit tests |
| Acting on a transient or partial state | Events are hints; a file that fails to parse keeps its last good routes and **nothing is deleted** while any file is unparsable; configuration changes are debounced | unit + end-to-end tests; mutation-checked |
| Stale in-memory view of the collection | Every replace/delete re-verifies the file hash; on mismatch the file is re-read and the plan recomputed once; user edits become conflicts | unit tests; mutation-checked |
| Partial files on shutdown | Writes are atomic; the loop finishes the batch in progress before stopping | end-to-end test |

## Known gaps

- Watch mode does not watch the collection: edits made in Bruno while it runs
  are noticed when a write would touch them (verification) or at the next full
  recomputation, not immediately.

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


## Phase 11 imported-source analysis — shipped subset and open gap

The shipped resolver (v0.2.0) reads TypeScript files selected by import
specifiers — untrusted source input. Satisfied by the shipped
implementation (containment and budget enforcement completed in Phase 11.1,
Milestone 11.1.1, 2026-11-10):

- lexical traversal (`..` escaping the project) is rejected; dependency
  resolution never escapes the project root lexically;
- **containment is enforced before any dependency read**
  (`resolve/containment.rs`): the project root is canonicalized once; the
  candidate path must be relative and free of dot components; **no path
  component may be a symlink** (rejecting symlinked targets, symlinked
  intermediate directories, and — since loops require following a link —
  symlink loops); and the final target must be a **regular file**.
  Directories, FIFOs, sockets, and devices are rejected (opening a blocking
  special file would stall the analyzer). Candidate selection uses
  `symlink_metadata`, never `Path::exists()`, so a symlinked candidate is
  selected deliberately and then rejected with a precise diagnostic instead
  of being silently followed;
- the existing source exclusion policy applies to imported files
  (`node_modules` and `.git` components are hard-skipped; configured
  exclude patterns apply); Controller discovery semantics unchanged;
- `node_modules` is never traversed, no packages are installed, and there
  is no network access;
- bounded work is enforced: four re-export hops, 512 dependency files,
  4,096 import edges, 4 MiB aggregate dependency bytes, 256 KiB per file,
  1,024 DTO declarations per run, 64 properties per DTO. All budgets share
  reject-the-exceeding-operation semantics (a module is analyzed completely
  or not at all); counters use checked/saturating arithmetic;
- source is parsed with Oxc only; TypeScript is never executed and modules
  are never resolved by invoking Node or `tsc`;
- an unreadable, rejected, or malformed dependency degrades to an unknown
  body shape, never to a route deletion or an invented body.

**Residual race (accepted, documented):** containment validation and the
read are separate operations; a concurrent process can swap a path
component between the two. The consequence is bounded to reading swapped
file content — nothing read is ever executed, written, or evaluated — and
perfect closure would require platform-specific open flags (`O_NOFOLLOW`)
that are deliberately not used. This is stated as defense in depth, not as
a guaranteed security boundary.

For Watch Mode, dependency events must invalidate dependants or trigger a
safe full resync (not yet wired: dependency-only changes are currently
ignored). For `sync --staged`, dependency contents must come only from the
captured Git index; working-tree fallback is forbidden — until an
index-only source provider exists, staged analysis intentionally resolves
same-file DTOs only. See
[Phase 11 security and handoff notes](phases/phase-11-cross-file-dto-resolution.md).
