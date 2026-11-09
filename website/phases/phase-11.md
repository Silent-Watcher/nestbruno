# Phase 11 — Cross-File DTO Resolution

<Badge type="info">Shipped in v0.2.0</Badge>

Phase 11 makes generated Bruno request bodies more complete by resolving
DTO declarations imported by NestJS Controllers. It covers both
traditional class/interface DTOs and a conservative subset of Zod schemas.

## Shipped scope

- On-demand resolution from imports of discovered Controllers; the
  TypeScript source tree is never scanned wholesale for DTO files.
- Local relative imports and a limited static `tsconfig.json` subset
  (`baseUrl`, `paths` with single-wildcard mappings), implicit `.ts` and
  `index.ts` resolution.
- No `node_modules` or arbitrary external package traversal.
- Re-export chains followed up to four hops; explicit `(module, symbol)`
  cycle detection; ambiguous exports fall back to `Unknown`.
- Exported class/interface DTOs with the existing 64-property cap, plus a
  conservative Zod subset: `z.object`, `z.infer`, `createZodDto` (import
  provenance required) and statically provable transformations (`extend`,
  `pick`, `omit`, `partial`, `required`, `strict`, `passthrough`, `strip`).
- Uncertain or ambiguous results stay `BodyShape::Unknown` — a partial
  property list is never published as a complete body.
- Bounded dependency processing: 512 files / 4 MiB aggregate per run,
  256 KiB per file, 1024 DTO declarations.

## Verification

- 535 tests pass, including resolver unit tests for module resolution,
  re-export chains, cycle detection, Zod transformations and tsconfig
  mapping (previously 495).
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings` clean; a dedicated Criterion benchmark
  (`extract_cross_file/1_controller_1_shared_dto`, ~40 µs) was added —
  see [Benchmarks](/guide/benchmarks).

## Deliberately deferred

The following remain open and are tracked in the repository's status and
known-issues documents:

- Canonical (symlink-safe) path containment for imported files —
  containment is currently lexical only.
- Run-scoped resolver caching (shared DTO files are re-read across routes).
- Watch Mode invalidation on dependency-only changes.
- Staged Sync dependency resolution from the Git index (staged analysis
  uses same-file DTOs only).

The detailed engineering record is maintained in the repository at
`docs/phases/phase-11-cross-file-dto-resolution.md`.
