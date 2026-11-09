# NestBruno Agent Onboarding

## Start here

1. Read `AGENTS.md` and `STATUS.md`.
2. Read `PROGRESS.md`, `ROADMAP.md`, `DECISIONS.md`, and `KNOWN_ISSUES.md`.
3. Read `docs/phases/phase-11-cross-file-dto-resolution.md` before touching
   cross-file DTO work. It is the canonical handoff for the current partial
   implementation and records the owner's approved architecture.
4. Inspect the current `git status` and preserve all pre-existing uncommitted
   work. Do not assume the archive starts clean.

## Product and architecture

NestBruno is a Rust 2024 / MSRV 1.91 CLI. It statically analyzes NestJS
TypeScript with Oxc 0.116 and generates/synchronizes Bruno `.bru` requests.

```text
Controller discovery → Oxc AST analysis → Route IR
    → Bruno model → serializer → safe writer / reconciler
```

Keep the library-first boundary. Route IR stays independent of Bruno and
TypeScript parser details. The generator never reads TypeScript. NestBruno
must never execute the target application or imported source.

## Current task: no active phase

Phases 11.1 (resolver completion), 12 (global prefix and URI versioning)
and 13 (query/param DTO resolution) are **complete, verified and shipped**:
v0.3.0, v0.4.0 and v0.5.0 (see `STATUS.md` and `docs/phases/
phase-11.1-resolver-completion.md`, `phase-12-global-prefix-versioning.md`,
`phase-13-query-param-dtos.md`). Phase 10 remains **OpenAPI
interoperability** and has not started — do not begin it without owner
authorization.

Accepted edges that still apply (tracked in `KNOWN_ISSUES.md`, must not be
silently changed):

- A validation/read race remains on concurrently modified dependency
  trees (containment is enforced, but check and read are separate
  operations; documented as defense in depth).
- Watch Mode does not watch the bootstrap file: `setGlobalPrefix` /
  `enableVersioning` edits need a watch restart.

Resolver scope for reference when touching it: on-demand resolution from
Controller imports only; relative imports (with NodeNext `.js`/`.mjs`/
`.cjs` mapping), limited `tsconfig` `baseUrl`/`paths`, directory
`index.ts`, and `package.json` `imports` local subpaths; no `node_modules`
or arbitrary packages; four re-export hops with explicit cycle detection;
exported class/interface DTOs plus the completed conservative Zod subset
(provenance-checked `z.infer` incl. cross-file and forward references,
multi-step chains, namespace `createZodDto`); unnamed `@Query()`/`@Param()`
object parameters resolve like bodies. Keep unsupported or ambiguous
shapes as `BodyShape::Unknown`; keep traversal bounded and Oxc ASTs
short-lived; preserve the 64-property limit and the enforced budgets
(reject-the-exceeding-operation).

## Current working tree

Clean as of the 2026-11-09 documentation pass. Phase 11 implementation was
committed as `d7bc701`; version 0.2.0 packaging as `3c0dbf5`.

## Quality and safety rules

- No `unsafe`, no production `unwrap()` / `expect()`.
- No regex-based TypeScript semantic extraction.
- No executing TypeScript/JavaScript, Node, `tsc`, NestJS, shell commands for
  Git, package managers, or network access from the analyzer.
- Do not weaken, skip, ignore, or remove existing tests to hide regressions.
- Do not change route identity, ownership stamps, reconciliation guarantees,
  or user-edited Bruno files.
- Keep `README.md` concise; detailed documentation belongs under `docs/`.
- Update `STATUS.md`, `PROGRESS.md`, the Phase 11 handoff, and any affected
  architecture/security/user documentation in the same work step.
- If an implementation task cannot be verified in the current environment,
  stop at the safe boundary and record exact findings, files, next action, and
  missing verification. Do not imply that code or tests passed.
- Do not create a Git commit unless the owner explicitly requests one.

## Required Rust quality gates

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo bench
```

Documentation website verification:

```bash
cd website
npm run build
```

If the Rust proxy fails with `unknown proxy name: 'ZCode-...'`, first try the
installed toolchain binaries directly as documented in `AGENTS.md`. If no
compatible toolchain exists, report the blocker rather than claiming tests
passed.
