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

## Current task: Phase 11

Phase 10 remains **OpenAPI interoperability**. Cross-file DTO support is
Phase 11 and is currently **partial / unverified**.

Owner-approved scope:

- Resolve dependencies on demand from imports of already-discovered
  Controllers; never scan the whole source tree for DTOs.
- Support local relative imports and limited `tsconfig` `baseUrl` / `paths`
  (simple wildcard mappings), explicit/implicit `.ts`, and `index.ts`.
- Do not traverse `node_modules` or resolve arbitrary packages.
- Follow at most four re-export hops; detect cycles explicitly.
- Support exported class/interface DTOs and a conservative Zod subset
  (`z.object`, aliases, `z.infer`, imported schemas, statically recognizable
  `createZodDto`, and safe transformations).
- Keep unsupported or ambiguous shapes as `BodyShape::Unknown`.
- Keep dependency traversal bounded, caches run-scoped, and Oxc ASTs short-
  lived. Preserve the existing 64-property DTO limit.
- Watch Mode and Git Staged Sync are in scope. Staged dependencies must come
  only from the index snapshot; never read a staged Controller with a
  worktree DTO.

Read the canonical Phase 11 handoff for the source-level blockers and exact
next-step sequence. Do not rely on the old Phase 10 proposal or the previous
version of this onboarding file; those contained conflicting or unverified
claims (including full-tree discovery, `node_modules`, 16-hop resolution,
1 MiB source files, and 1,024 properties).

## Current working tree

The supplied project snapshot contains pre-existing, uncommitted source
changes in `src/lib.rs`, `src/nest/extract.rs`, `src/nest/extract/tests.rs`,
`src/nest/mod.rs`, and untracked files under `src/resolve/`. Preserve and inspect
these changes; do not reset, clean, stash, or overwrite them. See the Phase 11
handoff for a precise inventory and known blockers.

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
