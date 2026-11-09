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

Completed after verification (2026-11-09):
- Cross-file resolution Criterion benchmark
  (`extract_cross_file/1_controller_1_shared_dto`, ~40 µs median,
  i7-1255U/Linux); recorded in `website/guide/benchmarks.md`.
- User-facing documentation: `website/guide/dto-resolution.md`,
  updated landing page, limitations, safety and phase pages.

### Post-release fix (2026-11-09)

- **NodeNext import specifiers.** Projects with
  `moduleResolution: "NodeNext"` must write the emitted extension in
  relative specifiers (`import { X } from './x.dto.js'`). `resolve/module.rs`
  only tried `<path>.ts` as-is, `<path>.ts` appended, and `index.ts`, so such
  specifiers resolved to nothing and every cross-file DTO degraded to
  `BodyShape::Unknown` (observed on a real NestJS monorepo: `z.infer` DTOs
  imported as `.js` produced empty `{}` bodies). Fixed by mapping
  `.js`/`.mjs`/`.cjs` specifiers to `.ts`/`.mts`/`.cts` candidates before the
  existing forms; unit tests added in `resolve::module::tests`. 539 tests
  pass; fmt and clippy `-D warnings` clean. Verified end-to-end on the same
  project (`sync` now emits the expected body skeletons).

### Phase 11.1 — Cross-file DTO resolver completion (started 2026-11-10)

Owner-authorized master prompt: six milestones completing Phase 11's
deferred work, then Phase 12 (global prefix/URI versioning) and Phase 13
(query/param DTOs). Version bump per completed phase, semver
(owner rule 2026-11-10). Milestones are committed individually.

#### Milestone 11.1.1 — Resolver security hardening and resource enforcement (completed 2026-11-10)

- **Containment** (`src/resolve/containment.rs`, new): the project root is
  canonicalized once at resolver construction (failure disables cross-file
  resolution with a warning); every dependency read is validated to be
  project-relative with no dot components, symlink-free at every component
  (rejects targets, intermediate directories, and loops), and a regular
  file at the final target (rejects directories and blocking/special
  files). `resolve/module.rs` candidate selection switched from
  `Path::exists()` to `symlink_metadata`, so symlink candidates are
  selected deliberately and rejected with a precise diagnostic instead of
  being followed silently (`exists()` hides loops behind `ELOOP`).
  Rejections degrade to `DtoResolution::Unknown`.
- **Budgets** (`src/resolve/limits.rs`, `src/resolve/mod.rs`):
  `MAX_IMPORT_EDGES` (4,096) counted per successful specifier resolution,
  the exceeding edge not followed; `MAX_DTO_DECLARATIONS` (1,024) rejects
  whole the module that would exceed it (was warning-only). All counters
  checked/saturating; boundary semantics documented: the operation that
  would exceed a budget is rejected in full; a module is analyzed
  completely or not at all.
- **Tests** (+20): containment unit tests (valid file, absolute/parent
  paths, missing, directory, socket; unix: symlink target outside root,
  symlinked intermediate dir, loop), resolver integration tests (symlink
  target/intermediate/loop → `Unknown` + diagnostic, directory and socket
  dependencies rejected, missing-then-created dependency recovers), and
  exact-boundary tests for every budget (files 512/513, bytes 4 MiB exact
  and +ε, per-file 256 KiB and +1, declarations 1,024 exact and +1,
  edges 4,096 and +1, re-export depth exactly 4).
- Gates: fmt + clippy `-D warnings` clean; 559 tests pass (was 539).
- Residual accepted race (validation and read are separate operations) is
  documented in the module doc and `docs/security.md`, not claimed closed.

#### Milestone 11.1.2 — Run-scoped resolver context, caching and single parsing (completed 2026-11-10)

- **Run-scoped context** (`ResolverContext` in `src/resolve/mod.rs`): module
  cache, all resource budgets, and diagnostics now live one-per-analysis-run
  (`analyze_each` creates it); `DtoResolver` became a cheap borrow holding
  only the cycle-detection stack. Dependency files are read and parsed once
  per run regardless of how many controllers or routes import them. Fresh
  context per run/watch-batch keeps the cache from going stale (pinned by
  `cache_does_not_leak_across_runs`).
- **Single parsing**: `zod::analyze_zod_program` analyzes an existing program
  (dependency modules parsed once, not twice); `seed_module_from_program`
  inserts the controller file's already-parsed AST metadata into the run
  cache, so the controller is never re-read/re-parsed for resolution;
  `ModuleAnalysis` dropped its retained source text. Cache keys are the
  normalized project-relative paths the module resolver produces; unreadable
  and malformed dependencies are not cached (repairs visible to retries,
  pinned by tests). Budgets remain per-run as documented; note this tightens
  them versus the old per-route counters (documented in CHANGELOG).
- **Pipeline threading**: the four optional parameters of
  `extract_controllers`/`analyze_content` became one `ProjectContext`
  carrying the resolver context; `run_analysis`'s closure bound relaxed
  `Fn + Send + Sync` → `FnMut + Send` (single worker thread).
- **Benchmarks**: new `analyze_one_run` harness; added
  `extract_cross_file/8_controllers_1_shared_dto` (79 µs),
  `1_controller_8_routes_shared_dto` (132 µs), `4_hops_reexport_chain`
  (26 µs), `8_controllers_8_dtos_each` (569 µs, 64 dependency files).
  Comparison against a fresh pre-change baseline (Criterion, same machine):
  `extract_cross_file/1_controller_1_shared_dto` 40.4 µs → 30.2 µs median
  (−35.3%, the eliminated second parse). A back-to-back stash A/B of the
  unchanged no-context path `extract/100_controllers` measured 533 µs →
  554 µs (+3.9%) with identical logic on that path — attributed to
  code-layout shift and recorded in the CHANGELOG rather than claimed as
  noise.
- Tests: +3 (cross-controller cache sharing with budget accounting, no
  cross-run cache leakage, malformed-then-repaired dependency retry).
  Gates: fmt + clippy `-D warnings` clean; 562 tests pass (was 559).

#### Milestone 11.1.3 — Dependency-aware Watch Mode (completed 2026-11-10)

- **Edge recording** (`src/resolve/mod.rs`): every `resolve_type` attributes
  the modules its specifier resolutions touch to the controller file
  (`ResolverContext::dependency_edges`); edges are recorded even when the
  module later turns out unreadable, so recreating it is observable. A
  binding whose specifier leads nowhere (deleted/renamed dependency) marks
  the controller's edge set **uncertain** (`edge_uncertain`).
- **Harvest & merge** (`src/workspace.rs::analyze_batch`): one fresh
  `ResolverContext` per batch (`analyze_each_with_context`); the recorded
  graph merges conservatively — failed analyses and uncertain edge sets
  keep the previous edges, everything else replaces wholesale (stale edges
  disappear when a controller stops importing a DTO).
- **Scheduling** (`Workspace::refresh`): dirty paths under the source tree
  are matched against the reverse index (exact and below-a-changed-
  directory); dependents join the analysis set. `is_dependency` lets the
  watch event pre-filter (`worth_checking`) keep dependency files that are
  not controller candidates.
- **Fallbacks preserved**: events remain hints; unknown files schedule
  nothing; overflow/errors keep the full-resync path (which rebuilds the
  index); debounce, last-good-state and incomplete-analysis protections
  untouched.
- **Tests** (+11): 7 workspace unit tests (dependent-only invalidation with
  untouched third controller, shared DTO, delete/recreate round trip,
  rename with import update, barrel re-export change, circular-dependency
  containment, deterministic incremental ≡ fresh-sync equivalence), 1 event
  pre-filter test, 3 real-binary e2e tests (DTO edit, shared DTO one-batch
  invalidation, delete/recreate). Gates: fmt + clippy `-D warnings` clean;
  573 tests pass (was 562).

#### Milestone 11.1.4 — Git-index source provider and staged cross-file resolution (completed 2026-11-10)

- **Provider boundary** (`src/resolve/provider.rs`, new): the resolver reads
  dependency content only through `SourceProvider` (`exists` + validated
  `read`). `DiskProvider` keeps the full containment discipline (canonical
  root, symlink-free components, regular file); candidate selection in
  `module.rs` now asks the provider instead of touching the filesystem.
  `ResolverContext::new` is disk-backed; `with_parts` takes any provider
  plus an explicit tsconfig.
- **Index provider** (`src/staged.rs`): `IndexSnapshot` captures stage-0
  regular-file entries (oid + mode) of the source tree plus the root
  `tsconfig.json` once; `IndexProvider` resolves reads by object id, so a
  later `git add` cannot mix into the run. Missing, conflicted,
  non-UTF-8, oversized, and non-regular (symlink 120000, gitlink) entries
  are not resolvable → conservative `Unknown`; symlink entries get a note
  diagnostic. `Repository` moves behind `Arc<Mutex<..>>` for the analysis
  worker (blob reads by id cannot drift).
- **Staged pipeline** (`analyze_staged`): the controller sources are
  analyzed with the index provider; the staged `tsconfig.json` (parsed from
  its blob) supplies `baseUrl`/`paths`; the extractor still seeds the
  controller's own parse. `sync --staged` gate widened: staged `.ts` files
  outside pruned dirs and `tsconfig.json` count as relevant changes, so a
  staged DTO edit alone updates its importers' bodies. No relevant staged
  change remains a clean no-op; the desired state is still the whole index.
- **Tests** (+13): index/worktree divergence (index version wins, worktree
  never leaks), staged DTO alone triggers and updates, unstaged worktree
  edit does not leak, staged dependency deletion degrades, staged re-export
  change propagates, symlink entry rejected with note, shared dependency
  serves both controllers, staged tsconfig paths mapping, gate tests
  (dependency `.ts` counts; data.json/dist/node_modules/README do not;
  tsconfig counts). Gates: fmt + clippy `-D warnings` clean; 586 tests
  pass (was 573).

#### Milestone 11.1.5 — Zod and module resolution completion (completed 2026-11-10)

- **Multi-step chains** (`src/resolve/zod.rs`): chains are collected to
  their innermost call and applied outward with per-call method extraction
  (the old code required every callee object to be an identifier, which
  silently marked any multi-call chain uncertain). Two supported bases: an
  inline `z.object({ ... })` (provides the base, the rest applies) and a
  known schema constant (every call applies). Chains starting at anything
  else (`z.string()...`) stay unresolved; unknown methods and non-literal
  pick/omit values stay uncertain. The master prompt's
  `z.object(...).pick(...).partial().required({...})` example is covered.
- **`z.infer` completion**: provenance-checked (the qualifier must be a
  real `zod` import — `Foo.infer` and bare `infer` are rejected);
  two-pass analysis lets the alias reference a schema declared later in
  the file; an imported/re-exported schema yields an import reference the
  resolver follows cross-file (same mechanism as `createZodDto`).
- **Namespace `createZodDto`**: `nz.createZodDto(...)` superclasses are
  recognized alongside named/aliased forms; unrelated namespaces are not.
- **`package.json` `imports`** (`src/resolve/packages.rs`, new): top-level
  `#` keys, exact and single-`*` wildcard patterns, local `./` string or
  array targets (first entry). Read through the source provider inside
  `ResolverContext::with_parts`, so disk and staged analysis both see
  their own package.json (the staged snapshot now captures it). External,
  `..`, and nested-condition targets are never resolved. Candidate
  selection then applies the full existing ladder (NodeNext `.js` mapping,
  `index.ts`, exclusions, containment).
- **`ModuleEnv`** (`resolve/module.rs`): the eight resolution inputs are
  bundled into one run-constant struct (clippy's argument-count lint led
  to a genuinely better shape).
- Tests: +12 (2 multi-step chains, forward `z.infer`, provenance
  rejections, namespace `createZodDto` + negative, 3 `PackageImports`
  unit tests, disk `#src` resolution with `.js` + external negative,
  staged `#src` resolution). Gates: fmt + clippy `-D warnings` clean;
  598 tests pass (was 586).

#### Milestone 11.1.6 — Resolver acceptance testing and documentation (completed 2026-11-10)

- **Pipeline acceptance** (`tests/cross_file.rs`, new, 9 tests): the real
  binary against real project shapes, asserting actual `.bru` output —
  class/interface/Zod `z.infer` DTOs, NodeNext `.js` specifiers,
  `package.json` `#src` subpaths, four-hop re-export chains, an unresolvable
  import staying `Unknown` with the explanatory docs line, update-in-place
  reconciliation on a DTO change (no rename, no recreate), and byte-identical
  repeated syncs.
- **Property tests** (+3): deterministic resolution and termination over
  random re-export chains with random cycles (`resolution_is_deterministic_
  and_terminates`); containment — no project-relative path input can make
  the validator return a path outside the canonical root
  (`containment_never_escapes_the_root`); a history of dependency-only
  edits (add/remove/rename fields, broken states, with two milestones of
  controller coverage already property-tested) leaves the incremental
  collection identical to a fresh full sync
  (`a_history_of_dependency_edits_gives_the_same_collection_as_a_fresh_sync`).
- **Documentation**: engineering record
  `docs/phases/phase-11.1-resolver-completion.md` (canonical handoff for
  the whole phase); ROADMAP, STATUS, KNOWN_ISSUES, CHANGELOG,
  architecture/security/project-status pages, and the website guides
  (dto-resolution, limitations, safety, commands) synchronized; website
  build passes.
- Gates: fmt + clippy `-D warnings` clean; 610 tests pass (was 598).

### Phase 12 — Global prefix and URI versioning (completed 2026-11-10, released 0.4.0)

- Bootstrap analysis (`src/nest/app.rs`): bounded discovery (optional
  `bootstrap` config key, then `src/main.ts`/`main.ts`); `setGlobalPrefix`
  with statically matchable exclude rules (exact/trailing-`*`, optional
  `{path, method}`); `enableVersioning` with URI provenance checks,
  `VERSION_NEUTRAL`, string/array defaults; non-URI strategies identified,
  never rendered as URLs.
- Composition in the extractor: `/{prefix}/v{n}/{controller}/{method}`,
  NestJS precedence, one route variant per declared version. No new IR
  fields. Without a bootstrap configuration the output is byte-identical
  to 0.3.0.
- Tests: 8 bootstrap unit tests + 7 real-binary acceptance tests
  (`tests/app_config.rs`). Record:
  `docs/phases/phase-12-global-prefix-versioning.md`. Accepted gap: watch
  does not watch the bootstrap file (KNOWN_ISSUES).

### Phase 13 — Query and param DTO resolution (completed 2026-11-10, released 0.5.0)

- `RouteParameter.fields`: resolved property names of unnamed
  `@Query()`/`@Param()` object parameters — same-file at extraction,
  cross-file in the existing resolver pass (gate widened beyond unknown
  bodies).
- Query fields become disabled, empty `params:query` entries, deduplicated
  against named ones; param fields are matched against URL-derived path
  parameters (URL stays authoritative; extra DTO fields documented, no
  invented placeholders).
- Tests: 3 end-to-end (`tests/cross_file.rs`). Record:
  `docs/phases/phase-13-query-param-dtos.md`.

### Phase 14 — CLI & CI developer experience (completed 2026-11-10)

Owner-authorized master prompt: four milestones (JSON output, read-only
verify, shell completions, GitHub Actions workflow template), one commit
each, no version bump and no releases during this phase (explicit prompt
rule). All four milestones are complete and committed
(`27c34a4`, `64ab454`, `26f3974`, `e7c71ea`).

### Phase 15 — Explain command (completed 2026-11-10)

Owner-approved master prompt (approval received for "Option A": reuse the
analysis results, no IR changes). Version bump to 0.6.0 per the
semver-per-phase rule (covers the unreleased Phase 14 features as well).
Not committed: the master prompt forbids commits unless explicitly
requested.

- `src/explain.rs` (new): `RouteSelector` parsing (`METHOD /path` via a
  clap value parser; case-insensitive method, composed-path
  normalization), route matching (exact method+path or
  controller/handler, optional file restriction with `./`-normalization),
  and the `Explanation` model: identification, derived `Resolution`,
  `limitations`, an honest findings checklist and the diagnostics of the
  controller's file plus the bootstrap warnings (which were previously
  computed but never surfaced).
- Resolution derivation (the one non-trivial part): the composed path is
  split back into `global prefix? / version? / controller path / handler
  path` using the same composition rules as the extractor
  (`compose_path` segment arithmetic); every candidate split is verified
  against `BootstrapConfig::prefix_applies_to` (exclude rules included)
  and `v<n>` version segments. Zero or several consistent splits become a
  limitation, never a guess. `nest::bootstrap_for` is now `pub(crate)`
  so explain uses the exact configuration of the analysis run.
- CLI: `nestbruno explain` with positional `"METHOD /path"`,
  `--controller`/`--handler` (clap `requires`/`required_unless_present`
  enforce the combination), `--file`, `--format json` (Phase 14
  convention, not a new `--json` flag). Errors: `Error::RouteNotFound` /
  `AmbiguousRoutes` (exit 1, candidates listed) and `Error::Usage` (exit
  2, new `exit_code::USAGE`); JSON mode prints the `found: false`
  document before the error.
- Tests: 14 unit (`src/explain.rs`: selector parsing, matching,
  ambiguity, derivation with prefix/exclude/version bootstrap fixtures,
  unresolved paths, trace content) + 14 integration (`tests/explain.rs`:
  full report, method/DTO metadata, bootstrap prefix+versioning, dynamic
  paths, missing/ambiguous selectors, usage errors, JSON validity and
  determinism, stdout/stderr separation). Gates: fmt + clippy
  `-D warnings` clean; 690 tests pass (was 662).
- Docs: CHANGELOG (0.6.0), STATUS, ROADMAP, `docs/project-status.md`,
  `DECISIONS.md` (rows 28–30), `website/guide/commands.md`.

#### Milestone 14.1 — Machine-readable JSON output (completed 2026-11-10)

- `src/output.rs` (new): presentation-only serde DTOs built from the
  existing route IR, `Discovery` and `SyncResult` — no duplicated logic.
  `write_json` emits exactly one pretty-printed document (plus newline) to
  stdout; serialization failures are `Error::Json` (exit 1).
- CLI: `OutputFormat` value enum (`text` default, `json`), a `--format`
  flag on `discover`, `routes` and `sync`. `ConflictReason::code()` adds a
  stable machine-readable name per conflict reason.
- Dependency: `serde_json` (companion of the existing `serde`; std has no
  JSON serializer).
- JSON contract: diagnostics are structured objects in the document;
  human diagnostics still go to stderr; staged changes are `staged_changes`
  entries; a no-op staged run emits a valid empty document; exit codes and
  the human format are unchanged.
- Tests: +10 integration (`tests/json.rs`): parseable single-document stdout
  for every command (including empty project and no-op staged runs),
  route/parameter/body/source fields, conflicts with reason codes, dry run
  predicting the real run, staged changes, incomplete analysis (unix-only)
  with diagnostics on stderr, invalid config producing no partial JSON,
  byte-identical repeated runs, and the default format staying human
  text. Plus 1 parse-level CLI unit test. Gates: fmt + clippy `-D warnings`
  clean; 640 tests pass (was 628).
- Docs: CHANGELOG (Unreleased), STATUS, `docs/project-status.md`,
  `website/guide/commands.md`.

#### Milestone 14.2 — Read-only collection verification (completed 2026-11-10)

- Shared pipeline extracted: `analyze_and_plan` in `src/commands.rs` runs
  the unchanged sync pipeline (analysis → generation → reconcile) for
  `sync` and the new `verify`; `verify` is its dry-run path plus reporting
  — no second reconciliation algorithm. Diagnostics are merged into
  `SyncResult.diagnostics` in report order, so sync's human output,
  stderr and JSON content are preserved.
- CLI: `nestbruno verify [--staged] [--format json]`; new
  `Outcome::OutOfSync` (exit 1, documented as the CI drift signal).
  Exit mapping: 0 in sync; 1 out of sync; 4 blocked (conflicts/failed
  operations, including critical conflicts) or incomplete (analysis could
  not establish the desired state); 3/1/2 via the existing `Error` path.
- Status categories follow the plan semantics: missing = would be created,
  outdated = would be updated, stale = would be deleted, conflicts and
  preserved per the ownership rules; human report shows the counts, the
  conflict/failed details and an explicit "The collection was not
  modified." footer.
- JSON: `status` (`in_sync`/`out_of_sync`/`blocked`/`incomplete`/
  `no_staged_changes`), `complete`, `summary`, `changes`, `conflicts`,
  `failed`, `diagnostics`; staged mode adds `staged_changes`; a no-op
  staged verify still emits a valid document.
- Tests: 15 integration (`tests/verify.rs`) — in-sync passes with a full
  project-tree comparison and an mtime equality check (no rewrite, not
  even in place); missing/outdated/stale detection with tree snapshots;
  user-modified requests reported as conflicts with content preserved;
  critical conflicts; unknown files preserved; incomplete analysis (unix)
  safe with nothing modified; empty project never bootstraps a collection;
  the required regression test proving verify never creates the collection
  directory; JSON validity/determinism; staged drift and staged no-op;
  invalid config prints no JSON. Plus 1 parse-level CLI unit test. Gates:
  fmt + clippy `-D warnings` clean; 656 tests pass (was 640).
- Docs: CHANGELOG (Unreleased), STATUS, `docs/project-status.md`,
  `website/guide/commands.md` (verify section + exit codes),
  `website/guide/synchronization.md` (verify vs `sync --dry-run`).

#### Milestone 14.3 — Shell completion generation (completed 2026-11-10)

- Dependency: `clap_complete = "4"` (the official clap companion; same
  major line as the existing clap 4). Generation comes from the real
  command tree — no hand-written per-shell scripts that could drift.
- CLI: `nestbruno completions <shell>` with `Shell` (Bash, Zsh, Fish,
  PowerShell, Elvish) as the value enum; documented and tested are
  bash/zsh/fish/powershell. Output goes exclusively to stdout through the
  existing BufWriter; nothing is written to disk, no shell configuration is
  touched, no shell process is spawned, generation is deterministic.
- Tests: 6 integration (`tests/completions.rs`): all four documented shells
  produce non-empty scripts naming the tool on stdout only, subcommand and
  option names appear (bash/zsh checked for `verify`, `sync`,
  `completions`, `--format`, `--dry-run`, `--staged`), byte-identical
  repeated generation, unknown shell → usage error (exit 2), missing
  shell argument → exit 2, `--help` lists the command. Gates: fmt + clippy
  `-D warnings` clean; 662 tests pass (was 656); `cargo build --release`
  passes (dependency change).
- Docs: CHANGELOG (Unreleased), STATUS, `docs/project-status.md`,
  `website/guide/commands.md` (completions section with per-shell setup),
  `website/guide/getting-started.md` (installation hint).

#### Milestone 14.4 — GitHub Actions verification workflow template (completed 2026-11-10)

- New guide page `website/guide/github-actions.md`, added to the VitePress
  sidebar: a copyable workflow that runs on `pull_request` with
  `permissions: contents: read`, checks out, installs a stable toolchain
  (`dtolnay/rust-toolchain@stable`, matching the repository's own CI
  conventions; MSRV-precise alternative documented), installs NestBruno
  from crates.io with `cargo install nestbruno --locked` (verified
  against the published 0.5.0 package with a local `--locked` install
  smoke test), and runs `nestbruno verify`.
- Documents: why Pull Requests fail on drift (exit 1/4 semantics),
  verification-only policy (no `sync` in CI, no repository modification,
  no secrets), JSON-report automation with `verify --format json`, and
  honest limitations (no `cargo install` caching via rust-cache, pinned
  action versions are the user's call, template not a Marketplace Action,
  not executed in this repository's CI).
- Verification performed: the embedded workflow YAML parses
  (`yaml.safe_load`); the JSON-report shape was validated against real
  `verify --format json` output of a drifted project (`status`,
  `summary`, `changes`); `cargo install --locked --path .` into a temp
  root succeeds and the binary runs. `jq` itself is not installed in this
  environment, so the `jq` expression was not executed — the JSON shape
  it reads was.
- Gates: fmt + clippy `-D warnings` clean; 662 tests pass; website build
  passes.
- Docs: CHANGELOG (Unreleased), STATUS, `docs/project-status.md`.

## Known gaps inside completed phases

See [KNOWN_ISSUES.md](KNOWN_ISSUES.md) for the verified list; the user-facing
summary lives in `docs/project-status.md` ("Current limitations").

## Documentation website (added 2026-11-06, after Phase 6)

`website/` holds a VitePress site with the product guide, landing page,
phase overview, and selected phase pages. Phases 9 and 11 are shipped and
described as such (DTO Resolution and Benchmarks guide pages added
2026-11-09). Keep website claims aligned with verified product behavior;
run `npm run build` in `website/` after content changes.
