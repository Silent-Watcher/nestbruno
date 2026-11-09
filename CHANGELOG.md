# Changelog

All notable changes to NestBruno are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- The Zod analysis subset is complete (Phase 11.1, Milestone 11.1.5):
  **multi-step transformation chains** applied in source order when the
  base is an inline `z.object({ ... })` or a schema constant (`extend`,
  `pick`, `omit`, `partial`, `required`, `strict`, `passthrough`,
  `strip`; anything unprovable stays unknown); **`z.infer<typeof S>`**
  with provenance checking (the `z` qualifier must come from a real `zod`
  import), forward references (the schema may be declared later in the
  file), and cross-file resolution of imported/re-exported schemas;
  **namespace-imported `createZodDto`** (`nz.createZodDto`) alongside
  named/aliased forms; and **`package.json` `imports` subpaths** — local
  `#src/...` specifiers resolve through exact and single-wildcard
  mappings, read through the active source provider (working tree or Git
  index snapshot), so staged analysis uses the staged `package.json`.
  External targets are never resolved; `node_modules` is never traversed.

- `sync --staged` now resolves cross-file DTOs against the Git index
  (Phase 11.1, Milestone 11.1.4): a source-provider boundary
  (`resolve::provider::SourceProvider`) separates the resolver from the
  filesystem, with a working-tree provider (full containment discipline)
  and a Git-index provider that reads only captured stage-0 blobs by object
  id. A staged controller resolves against the staged DTO — an index
  version is used even when the working tree holds different content, and
  an unstaged working-tree edit never leaks in. A staged `tsconfig.json`
  supplies `baseUrl`/`paths`; symlink and non-regular index entries are
  rejected (note diagnostic) and degrade to the conservative unknown shape.
- The `sync --staged` relevance gate now also fires for staged TypeScript
  dependency files and `tsconfig.json` (previously only controller
  candidates): a staged DTO edit alone updates the bodies of its importers.
  If nothing relevant is staged it remains a clean no-op, and the desired
  state is still reconstructed from the whole index.

### Fixed

- Watch mode now invalidates controllers on dependency-only changes
  (Phase 11.1, Milestone 11.1.3): every analysis records which dependency
  modules each controller's cross-file DTO resolutions touched, and watch
  keeps that reverse index. Editing, renaming, deleting, or recreating an
  imported DTO file re-analyzes exactly the controllers that import it —
  shared DTOs invalidate all their importers in one batch, and a recreated
  dependency is observed because the edge survives the unreadable period
  (an import whose specifier leads nowhere keeps the controller's previous
  edges). Files no analysis recorded as dependencies still trigger nothing;
  event loss and inconsistencies keep the existing full-resync fallback.
  Incremental results are pinned byte-equivalent to a fresh sync.

### Performance

- Cross-file DTO resolution is now run-scoped (Phase 11.1, Milestone 11.1.2):
  one resolver context per analysis run shares the module cache, resource
  budgets, and diagnostics across every Controller file and route. A
  dependency file is read and parsed once per run no matter how many
  Controllers import it; previously a fresh resolver per route re-read and
  re-parsed the controller and every dependency for each route.
- Single-parse module analysis: imports/exports, Zod schemas, and local DTO
  extraction all consume one oxc parse per module (previously dependency
  modules were parsed twice), and the controller file is never re-read for
  resolution — the extractor seeds the run cache with the AST-derived
  metadata of the program it already parsed. Measured: cross-file resolution
  benchmark 40 µs → 30 µs median (−35%, Criterion, same machine).
- `ModuleAnalysis` no longer retains the dependency source text; only
  compact AST-derived metadata is cached.
- Criterion suite extended with shared-workload cross-file benchmarks:
  `8_controllers_1_shared_dto` (79 µs), `1_controller_8_routes_shared_dto`
  (132 µs), `4_hops_reexport_chain` (26 µs), `8_controllers_8_dtos_each`
  (569 µs; 64 dependency files in one run).

### Changed

- The extraction pipeline threads a `ProjectContext` (root, source prefix,
  config, matcher, resolver context) instead of four separate optional
  parameters; `analyze_each` creates one `ResolverContext` per run. Staged
  analysis still runs without project context (same-file DTOs only, until
  the Git-index source provider lands).
- Measured on the unchanged no-cross-file extraction path
  (`extract/100_controllers`), a back-to-back A/B showed ~4% slower medians
  (533 µs → 554 µs) with identical logic; attributed to code-layout shift
  from the refactor and recorded here rather than claimed away.

### Security

- Cross-file dependency reads are validated before being opened
  (Phase 11.1, Milestone 11.1.1): the project root is canonicalized once;
  every path component must be symlink-free; the target must be a regular
  file. Symlinked targets, symlinked intermediate directories, symlink
  loops, directories, and special files (FIFOs, sockets, devices) are
  rejected with a diagnostic and degrade to the conservative
  `BodyShape::Unknown` — never partially read. Candidate selection uses
  `symlink_metadata` instead of `Path::exists()`, so symlinked candidates
  are rejected precisely instead of being followed. A validation/read race
  remains possible on a concurrently modified tree; nothing read is ever
  executed, and the limitation is documented rather than overstated.

### Fixed

- `MAX_IMPORT_EDGES` (4,096) is now enforced: each specifier that resolves
  to a project module consumes one edge, and the edge that would exceed the
  budget is not followed (previously the constant was declared but never
  counted).
- `MAX_DTO_DECLARATIONS` (1,024) is now enforced: the module that would
  push a run past the budget is rejected whole instead of only emitting a
  warning, so no partially tracked declaration set is published.
- Resolver counters use checked/saturating arithmetic, and all budgets now
  share documented reject-the-exceeding-operation boundary semantics
  (`src/resolve/limits.rs`).

## [0.2.1] — 2026-11-09

### Fixed

- NodeNext-style import specifiers resolve to their TypeScript sources:
  `./x.dto.js` now finds `x.dto.ts` (likewise `.mjs`/`.cjs` to
  `.mts`/`.cts`). Projects with `moduleResolution: "NodeNext"` previously
  got `BodyShape::Unknown` for every cross-file DTO because the resolver
  only tried `.ts`-suffixed candidates.

## [0.2.0] — 2026-11-09

### Added

- **Cross-file DTO resolution** (Phase 11): `@Body` DTO declarations are
  resolved across local TypeScript imports, not only the controller's own
  file. Supported: relative imports (implicit `.ts`, `index.ts`),
  `tsconfig.json` `baseUrl`/`paths` single-wildcard mappings, named /
  type-only / aliased imports, re-exports (four hops maximum), and exported
  class/interface DTOs with the existing 64-property cap.
- **Conservative Zod subset**: `z.object`, `z.infer<typeof Schema>`,
  `createZodDto` (import provenance required), and statically provable
  transformations (`extend`, `pick`, `omit`, `partial`, `required`,
  `strict`, `passthrough`, `strip`).
- Explicit `(module, symbol)` cycle detection and bounded dependency
  traversal (four re-export hops, 512 dependency files, 4 MiB aggregate
  source per run, 256 KiB per file). Uncertain or ambiguous resolution
  keeps the safe `BodyShape::Unknown` fallback — no guessed bodies.
- Criterion benchmark for cross-file resolution
  (`extract_cross_file/1_controller_1_shared_dto`, ~40 µs median).
- User guide pages: [DTO Resolution](website/guide/dto-resolution.md) and
  [Benchmarks](website/guide/benchmarks.md).

### Deferred (accepted limitations)

- Canonical (symlink-safe) path containment for imported files.
- Run-scoped resolver caching (shared dependency files are re-read per
  route); single-parse module analysis.
- Watch Mode invalidation on dependency-only changes.
- Staged Sync dependency resolution from the Git index (staged analysis
  uses same-file DTOs only).

## [0.1.1] — 2026-11-06

Patch release of the Phase 1–9 product: CLI (`init`, `check`, `discover`,
`routes`, `generate`, `sync`, `sync --staged`, `watch`), ownership-stamped
safe synchronization, incremental watch mode, Git staged synchronization,
production hardening with a Criterion benchmark suite.

## [0.1.0] — initial development releases

Phase-by-phase foundation through Phase 8 (see `docs/phases/`).
