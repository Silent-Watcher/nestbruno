# STATUS.md — current project snapshot

*Last updated: 2026-11-09, after completing Phase 11 implementation.*

## One-paragraph state

The product is at **v0.2.0** (commits `d7bc701` Phase 11 feature,
`3c0dbf5` version bump): a Rust 2024 / MSRV 1.91 CLI that statically
analyzes NestJS routes and keeps a Bruno collection synchronized.
Phases 1–9 and 11 are complete in the committed history. Phase 10 remains
**OpenAPI interoperability** and has not started. The Phase 11 working
tree was committed on 2026-11-09 after verification.

## Repository snapshot

- Phase 11 implementation: commit `d7bc701`.
- Version 0.2.0 + crates.io packaging (`include` list): commit `3c0dbf5`.
- Documentation overhaul (this file and the rest): the current working
  tree as of 2026-11-09.

## Current verified results (working tree, 2026-11-09)

- `cargo fmt --check` — pass
- `cargo clippy --all-targets --all-features -- -D warnings` — pass (0 warnings)
- `cargo test --all-features` — pass, 535 tests (was 495 at Phase 9)
- `cargo build --release` — pass

## What the current product state supports

| Command | Notes |
| --- | --- |
| `init` | writes `nestbruno.toml`, never overwrites |
| `check` | validates config + source directory |
| `discover` | lists Controller candidate files |
| `routes` | prints routes, params and source locations |
| `generate [--force]` | writes `.bru` requests with parameter metadata and cross-file DTO/Zod body skeletons |
| `sync [--dry-run]` | reconciles generated requests; preserves conflicts and user edits |
| `sync --staged [--dry-run]` | reconstructs desired state from the Git index for relevant Controller files |
| `watch [--debounce MS]` | incremental route sync with full-resync fallbacks |

**New in Phase 11 (shipped in v0.2.0):**
- Cross-file class/interface DTO resolution from imports in Controller files
- Cross-file Zod schema resolution including conservative transformation chains (extend, pick, omit, partial, required, strict, passthrough, strip)
- `z.infer<typeof Schema>` type aliases
- `createZodDto(Schema)` classes (including cross-file schema references)
- On-demand resolution bounded by: 4 re-export hops, 512 dependency files, 4 MiB total dependency bytes, 1024 DTO declarations (warning)
- Cycle detection via (module, symbol) resolution stack
- Uncertain/ambiguous results → `BodyShape::Unknown` (conservative)
- Benchmark `extract_cross_file/1_controller_1_shared_dto` — ~40 µs median (2026-11-09, i7-1255U)

## Phase 11 — remaining work (deferred to future phases)

1. Path containment/security checks (canonical symlink validation)
2. Make parsing/cache run-scoped (avoid duplicate parsing)
3. Watch Mode dependency invalidation
4. Staged Sync dependency resolution from Git index
5. Comprehensive integration/property tests for the resolver

Benchmarks and user-facing documentation were completed 2026-11-09
(`extract_cross_file` Criterion benchmark; `website/guide/dto-resolution.md`
and `website/guide/benchmarks.md`).

See `KNOWN_ISSUES.md` for the full source-level list and exact remediation
areas.

## Important decisions

- Phase 10 remains OpenAPI; cross-file DTO work is Phase 11.
- On-demand dependency traversal only; no whole-source-tree DTO scan.
- Local dependencies only; no `node_modules`/package resolution.
- `baseUrl` + simple `paths` wildcard subset; four re-export hops maximum.
- Class/interface DTOs and a conservative Zod subset share `BodyShape`.
- Unsupported or ambiguous resolution stays `BodyShape::Unknown`.
- No application execution and no modification of analyzed NestJS projects.
- No Git commits unless explicitly requested.
