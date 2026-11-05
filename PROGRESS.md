# PROGRESS.md — implementation state per phase

Status vocabulary: **Completed** (implemented + tests + docs + commit),
**Partial**, **Not Started**, **Blocked**.

Evidence basis: full code audit on 2026-11-06 at commit `48b201f`, plus a
green quality-gate run (fmt / clippy `-D warnings` / 443 tests). One clippy
drift from the newer toolchain (1.99.0) was found and fixed on that date; see
[KNOWN_ISSUES.md](KNOWN_ISSUES.md).

| Phase | Title | Status |
| --- | --- | --- |
| 1 | Project foundation | **Completed** |
| 2 | NestJS controller discovery | **Completed** |
| 3 | NestJS AST / route extraction | **Completed** |
| 4 | Bruno generation | **Completed** |
| 5 | Synchronization / reconciliation | **Completed** |
| 6 | Watch mode + incremental synchronization | **Completed** |
| 7 | Git staged synchronization | **Not Started** |
| 8 | Production hardening / release readiness | **Not Started** |
| 9 | Advanced route semantics & request metadata | **Not Started** |
| 10 | OpenAPI interoperability | **Not Started** |

Phase numbering note: the original conceptual plan (in the master prompt)
had dry run as Phase 9 and watch as Phase 7; the actual prompts reorganized
this — dry run shipped inside Phase 5, watch + incremental merged into
Phase 6. `docs/roadmap.md` records this mapping.

## Phase 1 — Foundation — Completed

- CLI (`init`, `check`; `sync`/`watch` were stubs at the time), library
  crate + thin binary, `nestbruno.toml` model with `deny_unknown_fields`,
  project root discovery (`nestbruno.toml` beats `package.json`), `Error`
  enum with exit codes 0/1/2/3/4, `Diagnostic`/`Reporter` on stderr.
- Files: `src/main.rs`, `src/cli.rs`, `src/commands.rs`, `src/config.rs`,
  `src/project.rs`, `src/error.rs`, `src/diag.rs`.
- Verified: unit + CLI integration tests, CI workflow.

## Phase 2 — Discovery — Completed

- Path-only candidate discovery (`Option A`): include/exclude globset
  (relative to `source`), `.ts` not `.d.ts`, `.git`/`node_modules`
  unconditional skip, depth cap 64, symlink entries → Note + never followed,
  source root containment check, deterministic sort.
- Files: `src/discovery.rs`. API: `discover_controllers(&Project)`.
- Verified: unit tests + 31 discovery integration tests incl. 10k-file tree,
  Unicode/spaces, symlink loops, unreadable dirs, invalid patterns.

## Phase 3 — Route extraction — Completed

- oxc 0.116 AST parser (chosen over alternatives; pins Rust 1.91), pure
  `extract_controllers(file, source)`; `@Controller` + `@Get/@Post/@Put/
  @Patch/@Delete/@Head/@Options/@All` + `@Param/@Query/@Body/@Headers`;
  route IR in `src/route.rs` (`Controller`, `Route`, `HttpMethod`,
  `RoutePath::{Static,Unresolved}`, `ParameterName`, source locations).
- Path composition via `compose_path` (idempotent, proptested).
- Safety: 256 KiB source cap, 512 MiB-stack worker thread, per-file panic
  guard, syntax error ⇒ no routes from that file + one error diagnostic.
- Dynamic decorator args → `Unresolved` + warning; never evaluated.
- Verified: 51 `#[test]` fns + 3 proptest suites in extract, 14 routes
  integration tests. Note: `docs/phases/phase-03.md` says "52 tests" —
  off by one; see [KNOWN_ISSUES.md](KNOWN_ISSUES.md).

## Phase 4 — Bruno generation — Completed

- `generate(config, controllers)` pure planner; one folder per controller,
  one `.bru` per route; kebab-case naming with collision suffixes
  (`-<8 hex>`); Windows-reserved names handled; `base_url` config key.
- Route identity (`bruno/identity.rs`): FNV-1a 64 fingerprints
  (`fingerprint`/`key`/`anchor`), values pinned by tests.
- Serializer refuses anything Bruno could misread (control chars, `{{`,
  `'''` openers, hostile names). Atomic writes, containment, symlink
  refusal (`bruno/writer.rs`). `generate --force` is the only overwrite
  path; nothing is ever deleted.
- Output format verified externally against Bruno's `@usebruno/lang`
  (documented in phase-04; not reproducible in-tree — see
  [KNOWN_ISSUES.md](KNOWN_ISSUES.md)).
- Verified: golden fixtures (`tests/fixtures/`), 21 generate integration
  tests incl. 500-route run, hostile-name injection, determinism.

## Phase 5 — Synchronization — Completed

- Ownership stamp in `docs` block; classification (generated-unmodified /
  generated-modified / unknown / invalid / malformed-metadata).
- Pure `reconcile::plan` with matching tiers (id → key → anchor), conflict
  taxonomy, deterministic alternative paths (`<stem>-<8hex>.bru`), adoption
  of byte-identical unstamped files; deletion only for unmodified generated
  files of removed routes and only when analysis is complete; critical
  conflicts write nothing.
- `sync`, `sync --dry-run` (same plan, zero writes, exit 0 with pending
  changes).
- Verified: 40 reconcile unit tests, 28 sync integration tests, 3 proptest
  suites (48 cases each) with 3 saved regressions in
  `proptest-regressions/sync/mod.txt`.

## Phase 6 — Watch + incremental — Completed

- `watch [--debounce MS]` (default 200, range 10..=60000): notify watcher,
  bounded channel (4096), event normalization, debounce with anti-starvation
  max-wait, overflow → full-resync fallback; per-directory non-recursive
  watches; config file debounced + reloaded (invalid config keeps old).
- `workspace.rs`: per-file analysis state (broken file keeps last-good
  routes; nothing deleted while incomplete), per-controller generation
  cache, in-memory collection index, one retry after stale-index re-read,
  bounded remembered diagnostics (1000).
- Shutdown: SIGINT/SIGTERM/SIGHUP → `Stopped.` + exit 0; closed stdout also
  stops; no temp files left.
- Verified: 20 watch e2e tests (Unix-only, no fixed sleeps) + unit/proptest
  coverage incl. "any history ≡ fresh sync" (40 histories).

## Known gaps inside completed phases

See [KNOWN_ISSUES.md](KNOWN_ISSUES.md) for the verified list; the user-facing
summary lives in `docs/project-status.md` ("Current limitations").

## Documentation website (added 2026-11-06, after Phase 6)

`website/` holds a VitePress site documenting everything through Phase 6:
landing page with an animated SVG logo (`prefers-reduced-motion` respected),
built-in local search, and 16 pages (9 guide pages, phases overview + 6
phase pages). Content is condensed from `docs/` and was checked against the
audit. Verified: `npm run build` green (dead links on), pages visually
inspected in a browser (home, guide, phase page, search interaction).
