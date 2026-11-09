# Changelog

All notable changes to NestBruno are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
