# Phase 11 — Cross-File DTO Resolution (Class + Zod)

**Status: Implemented and verified (2026-11-09).** All quality gates pass
(`cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo test --all-features` — 535 tests). Watch Mode dependency
invalidation and Staged Sync dependency resolution remain deferred; see
[STATUS.md](../../STATUS.md) for the current deferred list.  

> **Resolution addendum (2026-11-09).** Sections 3–8 below are the
> pre-verification handoff record, preserved for history. Outcome:
> implementation completed as commit `d7bc701` and shipped in v0.2.0
> (`3c0dbf5`). All P0 blockers were fixed and pinned by tests (Uncertain →
> `BodyShape::Unknown`; exported-binding-only resolution; aliased
> re-export mapping; explicit `(module, symbol)` cycle state; the
> bare-name `createZodDto` fallback removed). Resource budgets as shipped:
> 4 re-export hops, 512 dependency files, 4 MiB aggregate dependency
> bytes, 1,024 DTO declarations (warning), 256 KiB per file, 64
> properties. Still open from §4: canonical symlink containment, single
> parse per module, run-scoped resolver cache, Watch dependency
> invalidation, Staged index dependency provider, `MAX_IMPORT_EDGES`
> enforcement, multi-step Zod chains, and integration/property test
> coverage. A Criterion benchmark for cross-file resolution was added
> (`extract_cross_file/1_controller_1_shared_dto`, ~40 µs median on the
> benchmark machine; see `website/guide/benchmarks.md`).
**Scope authorization:** approved by the project owner on 2026-11-08 through the
Phase 11 master prompt and the accompanying architecture/implementation
authorization. Do not request approval again for decisions explicitly listed
below.  
**Phase numbering:** Phase 10 remains OpenAPI interoperability. This work is
Phase 11; do not rename or replace the Phase 10 roadmap item.

This document is the canonical handoff for the next agent. It records the
approved scope, the state of the supplied working tree, verified source-level
findings, and the next safe engineering steps. It deliberately does not claim
that the current implementation works: the Rust toolchain was unavailable in
the handoff environment, so no current build or test result could be verified.

## 1. Objective

Resolve request-body DTO declarations referenced by NestJS Controller
parameters, including declarations in other local TypeScript modules, and map
their top-level property names to the existing `BodyShape` IR. Support both
traditional class/interface DTOs and a conservative subset of Zod schemas.
The Bruno generator remains unaware of TypeScript, import graphs, and Zod.

Expected result for `@Body() dto: CreateUserDto` is unchanged at the IR boundary:

- successful static resolution → `BodyShape::Fields([...])`;
- unresolved, ambiguous, unsupported, malformed, or resource-limited input →
  `BodyShape::Unknown`;
- no body parameter → `BodyShape::None`.

All generated example property values remain `null`. This is not JSON Schema,
OpenAPI, or runtime validation inference.

## 2. Approved decisions (authoritative)

The accompanying implementation-authorization prompt supersedes the older
proposal that is still present in the original handoff history.

1. **On-demand resolution only.** Keep the current Controller discovery
   mechanism. Starting from discovered Controllers, inspect only imports
   reachable from the Controller and relevant to resolving a body type. Do not
   scan every `.ts` file, and do not turn DTO files into Controller candidates.
2. **Local modules only.** Support relative imports and the approved limited
   `tsconfig.json` subset: `baseUrl`, `paths`, and simple one-wildcard mappings.
   Resolve explicit/implicit `.ts` and directory `index.ts` forms. Parse JSONC
   statically. Do not implement `extends`, `rootDirs`, arbitrary package
   resolution, `package.json` exports, or `node_modules` traversal in this
   phase.
3. **No application execution.** Never run Node, `tsc`, NestJS, TypeScript,
   JavaScript, imported modules, package managers, subprocesses, or network
   requests. Oxc is the parser. Do not use regex for semantic extraction.
4. **Four re-export hops.** The maximum is four re-export edges, represented by
   an internal constant, not a public config setting. Define the count
   consistently and test the exact boundary and one-over-boundary case.
5. **Cycle handling.** Track resolution state by module and symbol (for example,
   `Unvisited` / `Visiting` / `Resolved`). A depth limit is not a substitute for
   explicit cycle detection.
6. **Class DTOs.** Preserve same-file extraction and the existing 64-property
   cap/order. Support exported classes/interfaces, named and type-only imports,
   aliases, and named re-exports. A declaration must actually be exported by
   the module through which it is imported.
7. **Zod DTOs.** Statically recognize imported Zod identifiers and the approved
   `nestjs-zod` `createZodDto` helper. Support `z.object`, aliased namespace or
   named `object` imports, `z.infer<typeof Schema>`, imported/re-exported
   schemas, and only those transformations whose property set can be
   established conservatively: `extend`, `pick`, `omit`, `partial`, `required`,
   `strict`, `passthrough`, and `strip`. Do not execute chained methods.
8. **No guessing.** Ambiguous symbols, unsupported expressions, unresolved
   aliases, and uncertain transformations must fall back to `BodyShape::Unknown`.
   Do not use a partial/uncertain property list as if it were complete.
9. **Bounded work.** Enforce explicit limits for dependency modules, import/
   export edges, aggregate dependency bytes, DTO declarations, source bytes,
   re-export depth, and properties. Keep caches bounded to one analysis run;
   do not retain Oxc ASTs after compact metadata extraction.
10. **No unrelated changes.** Keep Route IR and Bruno generation boundaries,
    route identity, ownership stamps, reconciliation, and user-edit protection
    unchanged. Do not add files, configuration, dependencies, decorators, or
    generated artifacts to the NestJS application being analyzed.
11. **Watch and Staged Sync are in scope.** DTO/schema changes must invalidate
    every dependent Controller in Watch Mode. Staged analysis must resolve
    dependencies exclusively from the Git index snapshot; never fall back to
    working-tree content.
12. **No commits** unless the owner explicitly requests them.

## 3. Current supplied working-tree state

The supplied archive is at commit `c95e4ab` (`bump version up`) with an
uncommitted working tree. These pre-existing changes were present before this
handoff and must be preserved until understood:

- Modified: `src/lib.rs`, `src/nest/extract.rs`,
  `src/nest/extract/tests.rs`, `src/nest/mod.rs`.
- Untracked: `src/resolve/{imports,limits,mod,module,tsconfig,zod}.rs`.
- Untracked handoff artifacts: `AGENT_ONBOARDING.md` and the old
  `docs/phases/phase-10-cross-file-dto-architecture.md`.

The old Phase 10 architecture proposal is superseded. It proposed a full TS
source discovery pass, package/`node_modules` resolution, and a 16-hop limit;
all three conflict with the approved decisions above. That untracked proposal
was removed from the active docs and replaced by this Phase 11 record. Phase 10
OpenAPI remains untouched in `ROADMAP.md`.

### Existing partial implementation

- `src/resolve/imports.rs`: extracts imports and some local/re-export forms.
- `src/resolve/module.rs`: resolves relative imports and limited `tsconfig`
  aliases.
- `src/resolve/tsconfig.rs`: parses a subset of `tsconfig.json` using Oxc.
- `src/resolve/zod.rs`: contains an initial Zod AST analyzer.
- `src/resolve/mod.rs`: module cache, resource constants, symbol lookup, and
  resolver entry point.
- `src/nest/extract.rs`: invokes cross-file resolution after same-file body
  extraction.
- `src/nest/mod.rs`: passes filesystem project context to disk-based analysis.
- `benches/pipeline.rs`: existing extraction benchmarks now pass an explicit
  empty dependency context after the extractor API change; these benchmarks
  have not been compiled in this environment.

These files are scaffolding, not evidence of feature completion. There are no
verified end-to-end DTO dependency tests, Watch Mode invalidation, or staged
index dependency resolution in the supplied changes.

## 4. Verified source-level blockers

The following findings were established by inspecting the supplied source.
They must be fixed and covered by tests before the feature is described as
working.

### P0 — correctness and security

- **Uncertain Zod fallback — source correction made, verification pending.**
  The supplied code previously mapped `DtoResolution::Uncertain(props)` to
  `BodyShape::Fields(props)`. This handoff changes that branch to keep the
  existing `BodyShape::Unknown` fallback. It has not been compiled or tested;
  add/verify an extractor-level regression before considering it resolved.
- **Module containment is lexical only.** `src/resolve/module.rs` uses
  `Path::exists()` and checks for lexical `..`, but does not reject absolute
  candidates or verify canonical path containment / symlinks / regular-file
  type. The comment says canonicalization is checked, but the implementation
  does not do so. An imported symlink can therefore point outside the project.
  Reuse the repository's existing filesystem-safety patterns and test
  symlinks, directories, special files, absolute paths, Windows drive/UNC
  forms, and canonical containment.
- **Re-export resolution passes an absolute importer path.**
  `lookup_export()` calls `resolve_imported_module()` with
  `project_root.join(module_path)`, while `resolve_module()` treats
  `from_file` as project-relative. Normalize the contract so every resolver
  path has one explicit representation (prefer project-relative paths) and
  never mix absolute and relative paths.
- **Re-export aliases are not resolved correctly yet.** The lookup logic must
  map the public/exported name to the source/local name. For
  `export { Original as Public } from './module'`, a request for `Public` must
  look up `Original` in the target module. Verify Oxc 0.116's AST fields and
  TypeScript semantics rather than copying the existing test's possibly
  reversed expectation.
- **Non-exported declarations can be resolved.** `lookup_export()` checks
  `local_dtos` and `zod_schemas` before proving the declaration is exported.
  An imported symbol must resolve only through a real exported binding, and
  conflicting/ambiguous exports must yield `Unknown`.
- **The Zod helper name-only fallback — source correction made, verification
  pending.** `src/resolve/zod.rs` previously accepted any bare callee named
  `createZodDto`. This handoff removes that fallback and adds a regression test
  requiring the recognized `nestjs-zod` import. Run the focused test and full
  suite before treating the correction as verified.

### P1 — pipeline, caching, and incremental behavior

- **Controller content is parsed/read twice.** `extract_controllers()` parses
  the controller, then `DtoResolver::find_import_binding()` calls
  `analyze_module()` and reads/parses the same Controller again. Reuse import
  metadata from the original AST analysis instead.
- **Zod dependencies are parsed twice.** `analyze_module()` parses with Oxc,
  then calls `analyze_zod_in_file()`, which creates another allocator and parses
  the same source again. Refactor Zod extraction to consume the existing Oxc
  `Program` so each module is parsed once.
- **The resolver cache is not shared across a run.**
  `resolve_cross_file_dtos()` constructs a new `DtoResolver`; the current call
  site invokes it per route. Shared DTO files are therefore re-read/re-parsed
  across routes and Controllers. Introduce a run-scoped analysis session/cache
  without retaining ASTs.
- **Watch Mode ignores dependency-only changes.** `Workspace::refresh()` only
  schedules a changed file when `Matcher::is_relevant()` returns true. That
  method intentionally means “Controller candidate” and is based on the
  configured include patterns. DTO files normally do not match these patterns.
  Add a reverse dependency index and invalidate dependent Controllers; when
  completeness cannot be proven, use the existing full-resync fallback.
- **Staged sync currently cannot resolve imported dependencies.**
  `analyze_contents()` calls `analyze_content()` with no project context, so
  cross-file resolution is disabled for `analyze_sources()`. In addition,
  `staged::collect_sources()` only reads entries satisfying
  `Matcher::is_relevant()`, i.e. Controller candidates. Build a staged source
  provider that resolves each dependency from the Git index on demand. It must
  never fall back to the working tree, even when a blob is absent from the
  index.
- **No verified reverse dependency graph exists.** Do not claim affected-only
  invalidation until tests prove that additions, removals, path changes,
  barrels, aliases, shared dependencies, and recreated files are all covered.

### P1 — resource limits and module analysis

- `MAX_IMPORT_EDGES` is defined but no edge counter/enforcement was found.
- `MAX_DTO_DECLARATIONS` currently adds a warning after the count exceeds the
  limit but does not stop or cap extraction.
- The `ModuleAnalysis` retains a cloned source `String` that is not used by
  current lookup code. Avoid retaining source buffers when compact AST-derived
  metadata is sufficient.
- The per-run byte/file limits need to include every parsed dependency once,
  use checked arithmetic, and have tests at and just beyond each boundary.
- The re-export depth limit currently bounds recursion, but the explicit
  `(module, symbol)` cycle state required by the approved decision is missing.

### P1 — Zod extraction gaps

- `analyze_chained()` is defined but has no call site. The accepted
  `extend`/`pick`/`omit` transformations are therefore not wired through the
  expression analysis pipeline.
- `z.infer` recognition must be tied to a known imported Zod binding; do not
  accept any unqualified type name `infer`.
- The Zod analyzer currently discovers schema-like declarations without
  consistently proving they are exported. Export visibility must be handled
  by the shared module/symbol resolver.
- Test whether property names remain complete and deterministic through
  transformation chains. Unknown computed/dynamic constructs must produce
  `Unknown`, not a partial body.
- Keep nested object schemas nested conceptually: only top-level property names
  belong in `BodyShape::Fields`.

### P2 — diagnostics and test completeness

- The resolver currently returns `Unknown` for several failure paths without
  a diagnostic. Define diagnostic severity/message policy, but do not turn
  metadata-only uncertainty into an analysis `Error` that incorrectly blocks
  route deletion.
- The new resolution coverage visible in the supplied tree is primarily unit
  tests under `src/resolve/`. Add realistic integration tests that assert
  emitted `.bru` output, route identity stability, watch updates, and staged
  snapshot correctness.
- Add property tests for deterministic resolution, cycle termination, module
  cache reuse, containment, and incremental/full-sync equivalence.

## 5. Resource-limit snapshot

The current partial `src/resolve/limits.rs` declares:

| Constant | Current value | Current issue |
| --- | ---: | --- |
| `MAX_REEXPORT_HOPS` | 4 | Approved value; add boundary tests with clearly defined counting |
| `MAX_DEPENDENCY_FILES` | 512 | Candidate only; validate and document |
| `MAX_IMPORT_EDGES` | 4,096 | Declared but not enforced |
| `MAX_TOTAL_DEPENDENCY_BYTES` | 4 MiB | Candidate only; validate and document |
| `MAX_DTO_DECLARATIONS` | 1,024 | Warning only; not enforced |
| `MAX_SOURCE_BYTES` | 256 KiB | Must remain aligned with the existing source-size contract |
| `MAX_DTO_PROPERTIES` | 64 | Existing compatibility limit; preserve |

The authorization prompt mandates the four-hop cap and bounded resources, but
does not prescribe values for all other internal budgets. Confirm that candidate
values are sensible against actual projects and benchmarks; do not expose them
as public configuration. Ensure all limits are enforced, not merely declared.

## 6. Recommended implementation sequence

Do not attempt the whole master prompt as one task. Work in narrow vertical
slices and leave a runnable handoff after each one.

1. **Establish a runnable baseline.** Obtain the Rust 1.91-compatible toolchain
   and run `cargo fmt --check`, targeted `cargo test resolve::`, and then the
   existing full quality gates. Capture exact output. The supplied execution
   environment did not contain `cargo` or `rustc` on `PATH` or under the usual
   rustup toolchain directory, so no result is currently known.
2. **Correct resolver invariants before feature expansion.** Fix path identity,
   canonical filesystem safety, export visibility, re-export alias semantics,
   explicit cycle state, and `Uncertain` → `Unknown`. Add focused unit tests.
3. **Make parsing/cache run-scoped.** Pass import/export/Zod metadata extracted
   from the Controller's existing AST. Parse each dependency module once,
   analyze all metadata from that AST, and reuse bounded compact metadata
   across Controllers in one run.
4. **Complete a class DTO vertical slice.** Relative import → exported class or
   interface → `BodyShape::Fields` → generated `.bru` body. Add integration
   tests for direct, aliased, and barrel imports, plus route identity and
   deterministic output.
5. **Complete Zod separately.** Direct schema, aliased/named imports,
   `z.infer`, `createZodDto`, imported schemas and safe transformations. Keep
   unsupported dynamic cases conservative.
6. **Introduce an explicit source-provider boundary.** Disk analysis and Git
   index analysis must use the same resolver logic with different immutable
   source providers. For staged mode, dependency reads must come exclusively
   from stage-0 blobs in the captured index; no worktree fallback. If the
   architecture cannot support this safely in a small step, document it and
   leave staged DTO support blocked rather than ship mixed-state results.
7. **Integrate Watch Mode.** Persist a reverse dependency index for the current
   workspace snapshot, invalidate all dependent Controllers on dependency
   events, and full-resync on uncertainty. Test incremental output byte-for-
   byte against a fresh sync.
8. **Bench and document measured behavior.** Extend Criterion only after
   correctness. Update the website feature guide/landing page only when the
   corresponding functionality has passed tests. Never publish estimated
   latency, memory, or test counts.

## 7. Verification status of this handoff

- Repository snapshot inspected: commit `c95e4ab` plus uncommitted files.
- Static source inspection: completed for the new resolver modules, Nest
  extractor integration, analysis pipeline, Watch Mode invalidation, staged
  source loading, and current agent/project context files.
- Rust commands: **not run** — neither `cargo` nor `rustc` was available in
  the supplied environment.
- VitePress build: attempted with Node v22.16.0 / npm 10.9.2, but could not
  start because `website/node_modules` is absent (`vitepress: not found`).
- Benchmark: not run.
- Tests: current working-tree count/result is **unknown**. `STATUS.md` records
  495 passing tests for the prior Phase 9 snapshot; the old untracked
  `AGENT_ONBOARDING.md` claimed a different count and failures without
  reproducible command output. Do not treat that older count as current.
- Git commit: not created.

## 8. Completion checklist

Phase 11 is not complete until all are true:

- [ ] Direct, aliased, type-only, path-mapped, and re-exported class/interface
      DTO imports resolve to the correct exported symbol.
- [ ] Zod schemas and supported transformations resolve conservatively.
- [ ] Unresolved/ambiguous/unsupported cases remain `BodyShape::Unknown`.
- [ ] Same-file DTO behavior and the 64-property cap remain unchanged.
- [ ] Every relevant module is read and parsed at most once per run.
- [ ] Shared dependency metadata is cached and resource limits are enforced.
- [ ] Absolute paths, traversal, symlinks, special files, and root escapes are
      rejected safely.
- [ ] Watch Mode invalidates affected Controllers and falls back safely.
- [ ] Staged Sync uses only index blobs for Controller and dependency content.
- [ ] Full sync and incremental sync produce equivalent final collections.
- [ ] Unit, integration, property, regression, and golden tests pass without
      weakening existing assertions.
- [ ] Criterion results are reproducible and published with environment and
      input context.
- [ ] `cargo fmt --check`, Clippy `-D warnings`, `cargo test --all-features`,
      `cargo bench`, and `website/npm run build` are verified.
- [ ] Architecture, security, testing, troubleshooting, landing page, guides,
      phase pages, and agent context documents match the actual implementation.
