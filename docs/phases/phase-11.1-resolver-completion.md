# Phase 11.1 — Cross-file DTO resolver completion

Owner-authorized master prompt (2026-11-10): complete everything Phase 11
deferred, in six milestones, each with its own commit; then Phase 12 (global
prefix and URI versioning) and Phase 13 (query/param DTO resolution) follow
in the same arc. Phase numbering is unchanged: Phase 10 remains OpenAPI
interoperability and is not part of this arc.

Every milestone kept the quality gates green (`cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`,
`cargo test --all-features`) and the architectural invariants intact:
static analysis only, conservative `BodyShape::Unknown`, bounded work,
never-destroy-user-data, deterministic output.

## Milestone 11.1.1 — Resolver security hardening and resource enforcement

Commit `ba24cf0` (fix). New `src/resolve/containment.rs`: the project root
is canonicalized once; every dependency read is validated to be
project-relative, symlink-free at every component (targets, intermediate
directories, loops), and a regular file (directories, FIFOs, sockets and
devices are rejected — a blocking special file could stall the analyzer).
Candidate selection in `resolve/module.rs` switched from `Path::exists()` to
`symlink_metadata` (a bare `exists()` hides symlink targets and loops behind
`ELOOP`). Budgets: `MAX_IMPORT_EDGES` counted (was declared only),
`MAX_DTO_DECLARATIONS` rejects the exceeding module whole (was warning
only), checked/saturating arithmetic, one documented boundary semantics:
the operation that would exceed a budget is rejected in full; a module is
analyzed completely or not at all. The residual validation/read race is
documented as accepted (defense in depth, nothing executed), not claimed
closed. Tests: +20 (containment unit, integration rejections, exact-limit
boundaries for every budget).

## Milestone 11.1.2 — Run-scoped resolver context, caching, single parsing

Commit `3fbc30a` (perf). `ResolverContext` (module cache, budgets,
diagnostics) is created once per analysis run (`analyze_each`); the
`DtoResolver` became a cheap borrow holding only the cycle-detection
stack. Single parsing: `zod::analyze_zod_program` analyzes an
already-parsed program (dependencies parsed once, not twice), the
extractor seeds the run cache with the controller's own parse
(`seed_module_from_program`), and `ModuleAnalysis` dropped its retained
source text. Pipeline threading: the extractor's four optional parameters
became one `ProjectContext`; the worker closure relaxed to `FnMut + Send`
(single worker). Benchmarks (fresh pre-change baseline, Criterion):
cross-file resolution 40 µs → 30 µs (−35%); four shared-workload
benchmarks added (`8_controllers_1_shared_dto` 79 µs,
`1_controller_8_routes_shared_dto` 132 µs, `4_hops_reexport_chain` 26 µs,
`8_controllers_8_dtos_each` 569 µs). Honest note recorded in the
changelog: the unchanged no-context extraction path measured +3.9% in a
back-to-back stash A/B (533 → 554 µs), attributed to code-layout shift.
Tests: +3.

## Milestone 11.1.3 — Dependency-aware Watch Mode

Commit `13b89d6` (fix). Analyses record controller → dependency-module
edges (`ResolverContext::dependency_edges`); edges are recorded even when
a module turns out unreadable, so recreating it is observable. An import
specifier that leads nowhere marks the controller's edge set uncertain.
Watch harvests the graph per batch and keeps a reverse index: a dirty
dependency file re-analyzes exactly the controllers that import it
(exact and below-a-changed-directory matching); the event pre-filter
keeps known dependency files. Merge policy: failed analyses and
uncertain edge sets keep the previous edges (a recreated dependency is
seen on its next event); everything else replaces wholesale. Existing
fallbacks (event loss → full resync, debounce, last-good-state,
incomplete-analysis) untouched. Tests: +11 (unit, pre-filter, three
real-binary e2e, deterministic incremental ≡ fresh-sync equivalence).

## Milestone 11.1.4 — Git-index source provider and staged cross-file resolution

Commit `1cbd495` (feat). `src/resolve/provider.rs`: the resolver reads
dependency content only through `SourceProvider`; `DiskProvider` keeps the
containment discipline; candidate selection asks the provider. Staged
analysis uses `IndexProvider`: an `IndexSnapshot` captures stage-0
regular-file entries (oid + mode) once; reads go by object id through
`Arc<Mutex<Repository>>`, so a later `git add` cannot mix into the run.
Missing, conflicted, non-UTF-8, oversized, and non-regular (symlink
120000, gitlink 160000) entries are not resolvable → conservative
`Unknown`. The staged gate now also fires for staged `.ts` dependency
files and `tsconfig.json` — a staged DTO edit alone updates its
importers' bodies; the desired state remains the whole index. A staged
`tsconfig.json` supplies `baseUrl`/`paths` from its blob. Tests: +13
(index/worktree divergence, deletions, re-export changes, symlink
entries, shared dependencies, gate widening).

## Milestone 11.1.5 — Zod and module resolution completion

Commit `a7df79b` (feat). Multi-step Zod chains collected to the innermost
call and applied outward with per-call method extraction; supported bases:
an inline `z.object({ ... })` or a known schema constant; unknown methods
and non-literal pick/omit arguments stay uncertain. `z.infer` is
provenance-checked (the qualifier must come from a real `zod` import —
`Foo.infer` and bare `infer` are never mistaken for Zod), resolves forward
references (two-pass analysis), and follows imported/re-exported schemas
cross-file via import references. Namespace-imported `createZodDto`
(`nz.createZodDto`) is recognized; unrelated namespaces are not.
`package.json` `imports` subpaths (`src/resolve/packages.rs`): local `#`
keys, exact and single-`*` wildcard patterns, `./` string or array targets
(first entry), read through the active source provider — staged analysis
sees the staged `package.json`. External targets are never resolved.
`ModuleEnv` bundles the specifier-resolution inputs (clippy's
argument-count lint led to a genuinely better shape). Tests: +12.

## Milestone 11.1.6 — Acceptance testing and documentation synchronization

This milestone. Real-binary pipeline acceptance suite (`tests/cross_file.rs`,
9 tests): class/interface/Zod DTOs, NodeNext `.js` specifiers, `#src`
subpaths, four-hop re-export chains, update-in-place reconciliation on DTO
changes, unresolvable types stay `Unknown` with an explanation,
byte-identical repeated syncs. Property tests (+3): deterministic
resolution and termination over random re-export chains with random cycles;
containment never returns a path outside the canonical root; a history of
dependency-only edits (add/remove/rename fields, broken states) leaves the
incremental collection identical to a fresh full sync.

## Verification state (end of phase)

- 610 tests pass (`cargo test --all-features`), fmt and clippy
  `-D warnings` clean, `cargo package` verified for the phase release.
- Known-issue ledger: all Phase 11 deferred items resolved except the
  documented validation/read race (see `KNOWN_ISSUES.md`).
- Version: bumped per the owner's per-phase semver rule (see
  [release.md](../release.md)); features shipped in this phase justify the
  minor bump.
