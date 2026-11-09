# ROADMAP.md — remaining phases (10–13)

Based on the authoritative owner prompts and the actual state in
[PROGRESS.md](PROGRESS.md). Phases 1–9 are complete in the committed product
history. Phase 10 and Phase 11 are separate: do not overwrite or renumber the
OpenAPI work. The development sequence is Phase 11.1 → Phase 12 → Phase 13,
executed milestone by milestone with per-milestone commits; Phase 10 remains
OpenAPI interoperability and is not scheduled in that arc.

Each phase starts only within its authorized scope. Do not create commits
unless explicitly requested. Quality gates are required before any commit:
`cargo fmt --check`, Clippy with `-D warnings`, and
`cargo test --all-features`.

## Phase 10 — OpenAPI interoperability — Not Started

Export the existing route IR to OpenAPI (prefer 3.1), with optional
`openapi validate --spec <file>` comparison. Consume the route IR; never
reparse TypeScript. Keep deterministic output, conservative schemas, no
network access, no `$ref` fetching, and no execution. This roadmap item is
unchanged by Phase 11.

## Phase 11 — Cross-file DTO resolution (class + Zod) — Completed (2026-11-09)

The owner authorized Phase 11 on 2026-11-08. Implementation was verified on
2026-11-09 (535 tests, fmt + clippy `-D warnings` clean) and shipped in
v0.2.0 with a cross-file resolution benchmark
(`extract_cross_file/1_controller_1_shared_dto`, ~40 µs median).

Shipped scope:

- On-demand resolution from imports reachable from discovered Controllers;
  never scans the whole `.ts` tree for DTOs.
- Local relative imports and a limited static `tsconfig.json` subset:
  `baseUrl`, `paths`, simple `*` wildcard mappings, implicit/explicit `.ts`,
  and directory `index.ts` resolution.
- No external package or `node_modules` traversal. External imports that are
  not recognized semantic identifiers (such as known `zod` imports) resolve
  to `BodyShape::Unknown`.
- Four re-export hops maximum, explicit cycle detection, deterministic symbol
  resolution, and bounded dependency/resource budgets.
- Exported class/interface DTOs plus a conservative Zod subset (`z.object`,
  aliased/named imports, `z.infer`, imported/re-exported schemas,
  `createZodDto` with import provenance required, and safely understood
  transformations).
- Class and Zod extraction share the existing `BodyShape` contract. Unsupported
  or ambiguous results are `Unknown`, never guessed or partially accepted
  (`Uncertain` → `Unknown` is pinned by extraction tests).

Deferred to a future phase (tracked in [KNOWN_ISSUES.md](KNOWN_ISSUES.md)):

- Watch Mode invalidation on dependency-only changes.
- Staged Sync dependency resolution from the captured Git index only.
- Canonical (symlink-safe) path containment for imported files.
- Run-scoped resolver caching and single-parse module analysis.

The canonical engineering record is in
[`docs/phases/phase-11-cross-file-dto-resolution.md`](docs/phases/phase-11-cross-file-dto-resolution.md).
The user-facing guide is `website/guide/dto-resolution.md`.

## Phase 11.1 — Cross-file DTO resolver completion — In Progress (started 2026-11-10)

Completes the engineering work deferred from Phase 11, milestone by
milestone (owner-authorized master prompt, 2026-11-10):

1. **11.1.1 Resolver security hardening and resource enforcement** —
   completed 2026-11-10: canonical-root containment with symlink/regular-file
   validation (`resolve/containment.rs`), `MAX_IMPORT_EDGES` and
   `MAX_DTO_DECLARATIONS` enforced, checked arithmetic, boundary tests.
2. 11.1.2 Run-scoped resolver context, module caching, single parsing —
   completed 2026-11-10.
3. 11.1.3 Dependency-aware Watch Mode — completed 2026-11-10: recorded
   controller → dependency graph, reverse index in watch, dependent
   scheduling with conservative merge (failed/uncertain analyses keep their
   edges), e2e + equivalence coverage.
4. 11.1.4 Git-index source provider and staged cross-file resolution —
   completed 2026-11-10: `SourceProvider` boundary (disk / index), staged
   analysis resolves dependencies from the captured index only, widened
   staged gate (staged `.ts` dependencies and `tsconfig.json`).
4. 11.1.4 Git-index-only source provider; cross-file resolution for
   `sync --staged` (index/worktree divergence must resolve to the index
   version).
5. 11.1.5 Complete the Zod analyzer subset — completed 2026-11-10:
   multi-step chains (inline `z.object` or constant base), provenance-
   checked `z.infer` (forward references, cross-file imports),
   namespace-imported `createZodDto`, `package.json` `imports` local
   subpaths (`resolve/packages.rs`).
6. 11.1.6 Resolver acceptance (integration + property) tests and
   documentation synchronization.

A version bump follows at phase completion per the owner's semver rule
(fix/hardening → patch at minimum; shipped features may justify minor).

## Phase 12 — Global prefix and URI versioning — Not Started

Static extraction of `app.setGlobalPrefix()` and `VersioningType.URI`
versioning (application default, controller- and method-level
`@Version`, `VERSION_NEUTRAL`) composed in the route IR — not as string
replacement in the generator. Route identity must be preserved where a
safe update is possible; multiple version variants must stay
distinguishable. Non-URI versioning strategies are identified but never
silently interpreted as URI prefixes.

## Phase 13 — Query DTO and Param DTO resolution — Not Started

Extends the shared resolver (same context, cache, limits, source
providers) to unnamed `@Query()` and `@Param()` object parameters:
DTO-derived disabled query parameters, and param metadata matched
against URL-derived path parameters without ever inventing placeholders.
Named `@Query('x')`/`@Param('x')` semantics are preserved unchanged.

## Cross-phase notes

- Phase 6 documents a measurement-motivated candidate (restrict watch
  reconciliation to affected routes); it is not scheduled independently.
- Update `STATUS.md`, `PROGRESS.md`, `KNOWN_ISSUES.md`, `DECISIONS.md`,
  architecture/security docs, the Phase 11 handoff, and relevant website pages
  as implementation changes are verified.
