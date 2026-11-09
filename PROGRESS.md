# PROGRESS.md — implementation state per phase

Status vocabulary: **Completed** (implemented + tests + docs + commit),
**Partial**, **Not Started**, **Blocked**.

Evidence basis for Phases 1–9: historical audits and quality-gate runs recorded
in the relevant phase documents. The supplied Phase 11 working tree is
uncommitted and has not been compiled or tested in the latest environment; see
[STATUS.md](STATUS.md) and the canonical [Phase 11 handoff](docs/phases/phase-11-cross-file-dto-resolution.md).

| Phase | Title | Status |
| --- | --- | --- |
| 1 | Project foundation | **Completed** |
| 2 | NestJS controller discovery | **Completed** |
| 3 | NestJS AST / route extraction | **Completed** |
| 4 | Bruno generation | **Completed** |
| 5 | Synchronization / reconciliation | **Completed** |
| 6 | Watch mode + incremental synchronization | **Completed** |
| 7 | Git staged synchronization | **Completed** |
| 8 | Production hardening / release readiness | **Completed** |
| 9 | Advanced route semantics & request metadata | **Completed** |
| 10 | OpenAPI interoperability | **Not Started** |
| 11 | Cross-file class/interface + Zod DTO resolution | **Completed** |

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

## Phase 7 — Git staged synchronization — Completed (2026-11-06)

- `sync --staged [--dry-run]`: analyzes the Git **index** state through
  libgit2 (`git2` 0.21, `src/staged.rs`), never the working-tree version of
  a staged file. The gate: staged deltas filtered by the Phase 2 matcher;
  no relevant staged change ⇒ clean no-op. The state: desired routes
  reconstructed from **all** relevant index entries (correctness invariant
  tested byte-for-byte against a materialized staged project).
- Added/modified/deleted/renamed (rename via `diff.find_similar`, requests
  updated in place by the Phase 5 fingerprints); unborn HEAD = empty tree;
  nested project roots mapped; conflicted paths excluded with an error
  diagnostic (nothing deleted while incomplete).
- `nest/mod.rs::analyze_sources` — in-memory analysis on the same
  big-stack worker (the prompt's `SourceInput` hook).
- Never mutates Git (no index/HEAD writes, no hooks, no subprocess).
- Verified: 13 unit + 14 integration tests (470 total); fmt + clippy
  `-D warnings` green. Doc: `docs/phases/phase-07.md`.

## Phase 8 — Production hardening — Completed (2026-11-06)

- Benchmarks: `benches/pipeline.rs` (criterion) — extract ~193 µs / 100
  controllers, generate ~864 µs / 300 routes, discover ~4.0 ms / 1,000
  files, sync no-op ~4.6 ms / 750 requests. Numbers in
  `docs/phases/phase-08.md`.
- Lifecycle end-to-end test (`tests/lifecycle.rs`): every command in one
  flow, exit-code contract pinned.
- Hardening: single debounce constant; audit doc-drift fixed (phase-03
  count, phase-01 keys, phase-05 sample, "API" collection name, 64 KiB
  policy, `init --project`); dependency tree reviewed; release checklist
  (`docs/release.md`); Cargo keywords/categories; release build + install
  smoke passed.
- 471 tests, fmt + clippy green. Doc: `docs/phases/phase-08.md`.

## Phase 9 — Advanced route semantics & request metadata — Completed (2026-11-08)

- Extended the route IR (no duplicate models): `RouteParameter.type_name`
  (keyword or bare identifier only), `BodyShape` enum on `Route.body`
  (`None`/`Unknown`/`Fields`).
- Extractor: `extract_type_name()` for simple types; `dto_properties()`
  collects same-file class/interface property names (max 64, source order,
  computed keys skipped, methods/constructors ignored).
- Generator: `Fields` → JSON skeleton with `null` per DTO property; `Unknown`
  → `{}`; `None` → `body: none`. Named `@Body('field')` parameters merged with
  DTO fields. Type metadata recorded in `docs` as `@Kind(name): type TypeName`.
- Identity unchanged (controller+method+path+handler): metadata changes
  update in place (UPDATE, not DELETE+CREATE), verified by sync integration
  tests. Backward compatible with phases 4–6 output.
- Tests: 24 new extractor tests (types × DTO extraction × body shape), 4 new
  generator tests, 3 new sync integration tests. Golden fixtures regenerated.
  495 total tests (was 471).
- Verified: fmt + clippy `-D warnings` + all tests pass. Doc:
  `docs/phases/phase-09.md`.


## Phase 11 — Cross-file DTO resolution (class + Zod) — Completed (2026-11-09)

The owner authorized this work on 2026-11-08 as a distinct Phase 11 so the
existing Phase 10 OpenAPI roadmap item remains intact. Implementation completed
2026-11-09.

Implemented architecture:

- On-demand resolution from imports in discovered Controller files only.
- Local relative imports and limited static `tsconfig` `baseUrl` / `paths`
  mappings; implicit `.ts` and `index.ts` resolution.
- No `node_modules` traversal; bare specifiers without `paths`/`baseUrl`
  resolution deferred (returns Unknown).
- Four re-export hops maximum (`MAX_REEXPORT_HOPS=4`); explicit cycle
  detection via (module, symbol) resolution stack; ambiguous exports → Unknown.
- Exported class/interface DTOs: property names extracted (max 64, source order).
- Conservative Zod subset: `z.object()`, `.extend()`, `.pick()`, `.omit()`,
  `.partial()`, `.required()`, `.strict()`, `.passthrough()`, `.strip()`,
  `z.infer<typeof Schema>`, `createZodDto(Schema)` (including cross-file
  schema references via `import_ref`). Unsupported/ambiguous transformations
  → `BodyShape::Uncertain` or `Unknown`.
- Integration: `nest/extract.rs` calls `resolve_cross_file_dtos` after
  same-file DTO extraction; applies first successful resolution to route body.
- Quality gates: 535 tests pass (was 495), `cargo fmt`, `cargo clippy --all-targets --all-features -D warnings` clean.

Files added/modified:
- `src/resolve/mod.rs` — `DtoResolver` with cross-file lookup, cycle detection, re-export following, import_ref resolution
- `src/resolve/imports.rs` — Import/export AST extraction (OxC)
- `src/resolve/module.rs` — Module resolution (relative, tsconfig paths, baseUrl)
- `src/resolve/tsconfig.rs` — tsconfig.json JSONC parsing via OxC
- `src/resolve/zod.rs` — Zod schema static analysis with transformation chains
- `src/resolve/limits.rs` — Resource limits (MAX_REEXPORT_HOPS=4, MAX_DEPENDENCY_FILES, MAX_TOTAL_DEPENDENCY_BYTES, MAX_DTO_DECLARATIONS)
- `src/nest/extract.rs` — Integration point calling `resolve_cross_file_dtos`
- `src/nest/mod.rs` — Analysis pipeline with project context
- `src/route.rs` — `BodyShape` enum (existing, used by this phase)

Remaining work (deferred to future phases):
- Path containment/security checks (canonical symlink validation)
- Make parsing/cache run-scoped (avoid duplicate parsing)
- Watch Mode dependency invalidation
- Staged Sync dependency resolution from Git index
- Comprehensive integration/property tests
- Benchmarks and user-facing docs

## Known gaps inside completed phases

See [KNOWN_ISSUES.md](KNOWN_ISSUES.md) for the verified list; the user-facing
summary lives in `docs/project-status.md` ("Current limitations").

## Documentation website (added 2026-11-06, after Phase 6)

`website/` holds a VitePress site with the product guide, landing page,
phase overview, and selected phase pages. Phase 9 is shipped; Phase 11 is
listed as in-progress and must not be described as released. The latest
handoff did not run `npm run build`, so the current site build status is
unknown. Keep website claims aligned with verified product behavior.
