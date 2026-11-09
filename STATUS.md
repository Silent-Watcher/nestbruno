# STATUS.md — current project snapshot

*Last updated: 2026-11-09, after completing Phase 11 implementation.*

## One-paragraph state

The last verified committed product state is `c95e4ab` (Phase 9 complete): a
Rust 2024 / MSRV 1.91 CLI that statically analyzes NestJS routes and keeps a
Bruno collection synchronized. Phases 1–9 are complete in the committed
history. Phase 10 remains **OpenAPI interoperability** and has not started.
Phase 11 (Cross-file DTO resolution) has been **implemented and verified**
in the working tree as of 2026-11-09. All quality gates pass.

## Repository snapshot

- Base commit: `c95e4ab` (`bump version up`).
- Modified files: `src/lib.rs`, `src/nest/extract.rs`, `src/nest/extract/tests.rs`, `src/nest/mod.rs`, `src/resolve/` (new), `AGENTS.md`, `ARCHITECTURE.md`, `DECISIONS.md`, `KNOWN_ISSUES.md`, `PROGRESS.md`, `ROADMAP.md`, `STATUS.md`, `docs/architecture.md`, `docs/project-status.md`, `docs/roadmap.md`, `docs/security.md`, `benches/pipeline.rs`, `website/.vitepress/config.mts`, `website/guide/limitations.md`, `website/phases/index.md`, `website/phases/phase-11.md` (new), `docs/phases/phase-11-cross-file-dto-resolution.md` (new).
- Do not reset, clean, stash, or overwrite this in-progress work without
  explicit direction. No commit was created during this session.

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
| `generate [--force]` | writes `.bru` requests with parameter metadata and same-file DTO body skeletons |
| `sync [--dry-run]` | reconciles generated requests; preserves conflicts and user edits |
| `sync --staged [--dry-run]` | reconstructs desired state from the Git index for relevant Controller files |
| `watch [--debounce MS]` | incremental route sync with full-resync fallbacks |

**New in Phase 11 (working tree, not yet committed):**
- Cross-file class/interface DTO resolution from imports in Controller files
- Cross-file Zod schema resolution including conservative transformation chains (extend, pick, omit, partial, required, strict, passthrough, strip)
- `z.infer<typeof Schema>` type aliases
- `createZodDto(Schema)` classes (including cross-file schema references)
- On-demand resolution bounded by: 4 re-export hops, 50 dependency files, 10 MB total dependency bytes, 200 DTO declarations
- Cycle detection via (module, symbol) resolution stack
- Uncertain/ambiguous results → `BodyShape::Unknown` (conservative)

## Phase 11 — remaining work (deferred to future phases)

1. Path containment/security checks (canonical symlink validation)
2. Make parsing/cache run-scoped (avoid duplicate parsing)
3. Watch Mode dependency invalidation
4. Staged Sync dependency resolution from Git index
5. Comprehensive integration/property tests
6. Benchmarks and user-facing docs

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
