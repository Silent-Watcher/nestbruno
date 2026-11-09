# Phase 11 — Cross-File DTO Resolution

<Badge type="warning">In progress — not released</Badge>

Phase 11 aims to make generated Bruno request bodies more complete by
resolving DTO declarations imported by NestJS Controllers. It covers both
traditional class/interface DTOs and a conservative subset of Zod schemas.

## Approved scope

- Resolve only dependencies reachable from imports of discovered Controllers;
  do not scan the whole TypeScript source tree.
- Support local relative imports and a limited static `tsconfig.json` subset
  (`baseUrl`, `paths`, and simple wildcard mappings), plus `.ts` and `index.ts`
  resolution.
- Do not traverse `node_modules` or arbitrary external packages.
- Follow at most four re-export hops and detect cycles explicitly.
- Recognize exported class/interface DTOs and supported Zod patterns such as
  `z.object`, `z.infer`, `createZodDto`, and statically safe transformations.
- Keep ambiguous, unsupported, or uncertain results unknown rather than
  inventing fields.
- Keep dependency processing bounded and static: NestBruno must never execute
  the target application or imported source.
- Integrate dependency changes with Watch Mode and resolve staged dependencies
  exclusively from the Git index snapshot.

## Current state

The repository contains a partial Rust implementation under `src/resolve/`,
but it has not passed a complete verification cycle. Known work remains in
filesystem containment, re-export alias resolution, exported-symbol checks,
run-scoped caching, Zod analysis, resource-limit enforcement, Watch Mode
invalidation, Git Staged Sync, integration/property tests, and benchmarks.

Do not rely on this page as a compatibility guarantee until the phase is
completed and its quality gates pass. The detailed engineering handoff and
next-agent instructions are maintained in the repository at
`docs/phases/phase-11-cross-file-dto-resolution.md`.
