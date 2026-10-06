# Phase 2: NestJS controller discovery

## Objective

Determine, quickly and deterministically, which TypeScript files of a project
are candidates for containing NestJS controllers, without reading their
contents.

## Scope

Implemented: configured source root, directory traversal, pruning, `.ts`
filtering, include/exclude matching, deterministic sorted result, recoverable
error reporting, `nestbruno discover`.

Not implemented (later phases): reading or parsing files, route extraction,
Bruno, caching, watch, Git.

## Architecture

`src/discovery.rs`; public API `discover_controllers(&Project) ->
Result<Discovery, Error>`. See [architecture.md](../architecture.md) for the
pipeline diagram.

```rust
pub struct ControllerFile { pub path: PathBuf }   // relative to the project root
pub struct Discovery {
    pub files: Vec<ControllerFile>,       // sorted, unique
    pub diagnostics: Vec<Diagnostic>,     // recoverable problems
    pub directories_read: usize,
}
```

### Decision: Option A (candidates by configured patterns)

Discovery does **not** verify `@Controller`. Reasons: it keeps discovery to
one `read_dir` per directory with zero file reads; it avoids a second,
weaker "is this a controller" implementation next to the Phase 3 AST
analysis; and the AST pass already reads every candidate, so verification
there costs nothing extra. A candidate without a controller simply yields no
routes in Phase 3. Controllers in files not matching `include` are found by
widening `include`.

## Configuration

Existing Phase 1 keys are reused; no new configuration system.

- `source`: root of the walk. `include`: globs, at least one. `exclude`: globs.
- Patterns are matched against the path **relative to `source`**.
- `*` does not cross `/`; `**` does; case-sensitive everywhere.
- **Precedence:** included **and** not excluded. Exclusion wins.
- A directory matched by an exclude pattern (including patterns like
  `**/dist/**` that match everything beneath it) is not entered.
- Default `include`: `**/*.controller.ts`. Default `exclude`: `**/dist/**`,
  `**/build/**`, `**/coverage/**`, `**/.tmp/**`, `**/.cache/**`.
- `.git` and `node_modules` are always skipped and cannot be re-enabled.
- Only `.ts` files are candidates; `.d.ts` is not; `.tsx` and JavaScript are
  not (NestJS controllers do not use JSX, and the project targets TypeScript
  NestJS).

Because matching is relative to `source`, a legitimate source directory that
happens to be called `build` (or a project that lives under a directory called
`dist`) is not accidentally ignored. Only *nested* `build`/`dist`/… directories
inside the source tree are excluded by default. This is covered by tests.

## Error handling

| Situation | Policy |
| --- | --- |
| Invalid include/exclude pattern | Fatal, exit 3, names the pattern and which setting |
| Source missing / not a directory | Fatal, exit 3 |
| Source resolves outside the project (symlink) | Fatal, exit 3 |
| Source root unreadable | Fatal, exit 1 |
| Unreadable subdirectory / entry / unknown file type | `error:` diagnostic, continue, **result marked incomplete**, `discover` exits 4 |
| Directory deeper than 64 levels | `error:` diagnostic, not entered, incomplete |
| Symlink inside the tree | `note:` (shown with `--verbose`), not followed, not incomplete |

`Discovery::is_incomplete()` is true when any `Severity::Error` diagnostic
exists. Later phases must treat such a result as "state unknown", not "no
controllers", which matters for reconciliation (never delete Bruno requests
because discovery could not see a directory).

## Security

Discovery never writes, never executes anything, never reads file contents,
never spawns a process. Symlinks inside the tree are not followed (so loops
terminate and links cannot lead out of the project). The source directory may
itself be a symlink only when it resolves inside the project root. The walk
is iterative with a depth cap. See [security.md](../security.md).

## Performance

- Patterns are compiled once per run into two `GlobSet`s.
- A directory is listed at most once; excluded directories and
  `.git`/`node_modules` are never listed.
- The file-name extension is checked before any path is allocated; the
  relative path is only built for `.ts` files; one normalized `Candidate`
  serves both include and exclude matching.
- Sorting happens once at the end. No canonicalization inside the walk.
- No file contents are read. No cache (Phase 6).

Measurement (single ad-hoc run, Linux sandbox filesystem, warm cache, release
build): a synthetic tree of 100,000 files (50,000 under `src` in 1,000
directories, 50,000 under `node_modules`) produced 2,500 candidates, listed
1,001 directories (so `node_modules` was never entered), took roughly 30 ms
wall time, and peaked at about 11 MB RSS. This is an observation, not a
benchmark; the proper benchmark suite is Phase 10.

## Testing

Unit tests (`src/discovery.rs`): include/exclude semantics, precedence,
`*` vs `**`, case sensitivity, directory pruning (including partial excludes
that must *not* prune), extension rules, invalid patterns.

Integration tests (`tests/discovery.rs`, real temp trees; library and binary):
the specification's reference project, empty source, no controllers, deep
nesting, spaces, Unicode, non-UTF-8 names (unix), nested build directories,
pruning proven via `directories_read`, `.git`/`node_modules` with empty
`exclude`, `source = "."`, source named `build`, project under `dist/`,
custom include, exclude precedence, `.d.ts`/`.tsx`, sorted/unique/repeatable
output, depth limit, a 10,000-file project, missing/non-directory source,
invalid patterns via the CLI, symlinked directories/files/loops, source
symlinked outside/inside the project, unreadable subdirectory and unreadable
source root, CLI output format and exit codes.

The permission tests cannot observe anything as root; they were additionally
run as an unprivileged user (`setpriv`, uid 65534) and passed.

## Limitations

- Candidates only; no verification of `@Controller`.
- Pattern syntax is `globset`'s; no negation (`!pattern`), no `.gitignore` support.
- Case-sensitive matching even on case-insensitive filesystems.
- Windows behavior is unverified (CI is configured, not yet run).
- Symlinked source files/directories inside the tree are skipped, so
  controllers reachable only through symlinks are not found.

## Acceptance criteria

All items of the Phase 2 prompt's section 27 are met; see the test list above.

## Commits

See `git log`: `feat: add NestJS controller candidate discovery`,
`test: add controller discovery integration tests`, `docs: document phase 2`.

## What Phase 3 builds on

Phase 3 calls `discover_controllers(&project)?`, reads each
`ControllerFile` (relative to `project.root()`), parses it, and extracts
controllers and routes. It must propagate `Discovery.diagnostics` and honor
`is_incomplete()`.
