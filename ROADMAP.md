# ROADMAP.md — remaining phases (10–11)

Based on the authoritative owner prompts and the actual state in
[PROGRESS.md](PROGRESS.md). Phases 1–9 are complete in the committed product
history. Phase 10 and Phase 11 are separate: do not overwrite or renumber the
OpenAPI work.

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

## Phase 11 — Cross-file DTO resolution (class + Zod) — Partial / unverified

The owner authorized Phase 11 on 2026-11-08. The supplied source tree contains
an incomplete implementation attempt; it is not a shipped capability.

Approved scope:

- On-demand resolution from imports reachable from discovered Controllers;
  never scan the whole `.ts` tree for DTOs.
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
  `createZodDto`, and safely understood transformations).
- Class and Zod extraction share the existing `BodyShape` contract. Unsupported
  or ambiguous results are `Unknown`, never guessed or partially accepted.
- Watch Mode invalidates all dependent Controllers. Staged Sync resolves all
  dependencies from the captured Git index only, never the working tree.
- No changes to the analyzed NestJS project; resolver code lives in this Rust
  repository.

The canonical handoff, inspected working-tree state, verified blockers, and
recommended narrow implementation sequence are in
[`docs/phases/phase-11-cross-file-dto-resolution.md`](docs/phases/phase-11-cross-file-dto-resolution.md).

Historical Phase 9 quality gates passed with 495 tests. The modified Phase 11
working tree has not yet been verified in a Rust-enabled environment. No
Phase 11 benchmark figures or test counts may be published until measured.

## Cross-phase notes

- Phase 6 documents a measurement-motivated candidate (restrict watch
  reconciliation to affected routes); it is not scheduled independently.
- Update `STATUS.md`, `PROGRESS.md`, `KNOWN_ISSUES.md`, `DECISIONS.md`,
  architecture/security docs, the Phase 11 handoff, and relevant website pages
  as implementation changes are verified.
