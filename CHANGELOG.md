# Changelog

All notable changes to NestBruno are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the project adheres to
[Semantic Versioning](https://semver.org/).

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
