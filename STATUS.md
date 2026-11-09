# STATUS.md — current project snapshot

*Last updated: 2026-11-10, v0.5.0 + Phase 14 complete (JSON output, verify, completions, CI workflow template).*

## One-paragraph state

The product is at **v0.5.0** with Phases 11.1, 12, 13 complete, and
Phase 14 (CLI & CI developer experience) complete per the owner-authorized
master prompt of 2026-11-10: 14.1 `--format json`, 14.2 read-only `verify`,
14.3 shell completions, 14.4 a GitHub Actions workflow template — each
committed separately, no version bump during Phase 14 (explicit
master-prompt rule for this phase). Milestone 11.1.1: dependency reads are gated by
canonical-root, symlink-free, regular-file containment and all resolver
budgets are enforced. Milestone 11.1.2: cross-file resolution is run-scoped —
one `ResolverContext` per analysis run shares the module cache across
Controllers and routes, every module is parsed once (the extractor seeds the
cache with the already-parsed controller program), and the Criterion suite
gained four shared-workload benchmarks (cross-file resolution 40 µs → 30 µs).
Milestone 11.1.3: watch mode keeps the reverse dependency index recorded by
each analysis and re-analyzes the dependent controllers on dependency-only
changes (edit, rename, delete, recreate, shared DTOs, barrels, cycles) with
the conservative merge policy and the existing full-resync fallback.
Milestone 11.1.4: a `SourceProvider` boundary separates the resolver from
the filesystem; `sync --staged` resolves cross-file DTOs from the captured
Git index only (never the working tree), and its gate now fires for staged
dependency files and `tsconfig.json` too. Milestone 11.1.5: the Zod subset
is complete — provenance-checked `z.infer` (forward references and
cross-file schemas), multi-step transformation chains, namespace-imported
`createZodDto`, and `package.json` `imports` local subpaths.
Phases 1–9 and 11 are complete in the committed history; Phase 10 remains
**OpenAPI interoperability** and has not started.

## Repository snapshot

- Phase 11 implementation: commit `d7bc701`.
- Version 0.2.0 + crates.io packaging (`include` list): commit `3c0dbf5`.
- Documentation overhaul + NodeNext fix + v0.2.1 bump: 2026-11-09.
- Phase 11.1 (Milestones 11.1.1–11.1.6, commits `ba24cf0`..`cef4a80`)
  + v0.3.0 bump: the current working tree as of 2026-11-10.

## Current verified results (working tree, 2026-11-09)

- `cargo fmt --check` — pass
- `cargo clippy --all-targets --all-features -- -D warnings` — pass (0 warnings)
- `cargo test --all-features` — pass, 662 tests (656 after Phase 14.2; +6 for Phase 14.3)
- `cargo build --release` — pass

## What the current product state supports

| Command | Notes |
| --- | --- |
| `init` | writes `nestbruno.toml`, never overwrites |
| `check` | validates config + source directory |
| `discover` | lists Controller candidate files; `--format json` (Phase 14.1) |
| `routes` | prints routes, params and source locations; `--format json` |
| `generate [--force]` | writes `.bru` requests with parameter metadata and cross-file DTO/Zod body skeletons |
| `sync [--dry-run]` | reconciles generated requests; preserves conflicts and user edits; `--format json` |
| `sync --staged [--dry-run]` | reconstructs desired state from the Git index for relevant Controller files; `--format json` |
| `verify [--staged]` | read-only drift check reusing the sync pipeline as a dry run; exit 0/1/4; `--format json` |
| `completions <shell>` | prints a completion script (bash/zsh/fish/powershell) to stdout |
| `watch [--debounce MS]` | incremental route sync with full-resync fallbacks |

**New in Phase 14 (complete, unreleased):**
- 14.1 done: `discover`/`routes`/`sync` accept `--format json` and print
  exactly one JSON document to stdout (diagnostics included as structured
  objects; human text stays the default; exit codes unchanged).
- 14.2 done: `verify` (with `--staged`) checks the collection against the
  sources strictly read-only; exit 1 when out of sync, 4 on conflicts or
  incomplete analysis; shares the sync pipeline, never duplicates it.
- 14.3 done: `completions bash|zsh|fish|powershell` generated from the
  command tree via `clap_complete`; stdout only, nothing installed.
- 14.4 done: GitHub Actions verification workflow template documented in
  `website/guide/github-actions.md` (pull_request, `contents: read`,
  `cargo install nestbruno --locked`, `nestbruno verify`).

**New in Phase 11 (shipped in v0.2.0):**
- Cross-file class/interface DTO resolution from imports in Controller files
- Cross-file Zod schema resolution including conservative transformation chains (extend, pick, omit, partial, required, strict, passthrough, strip)
- `z.infer<typeof Schema>` type aliases
- `createZodDto(Schema)` classes (including cross-file schema references)
- On-demand resolution bounded by: 4 re-export hops, 512 dependency files, 4 MiB total dependency bytes, 1024 DTO declarations (warning)
- Cycle detection via (module, symbol) resolution stack
- Uncertain/ambiguous results → `BodyShape::Unknown` (conservative)
- Benchmark `extract_cross_file/1_controller_1_shared_dto` — ~40 µs median (2026-11-09, i7-1255U)

**Post-release fix (v0.2.1, 2026-11-09):**
- NodeNext emitted specifier extensions (`.js`/`.mjs`/`.cjs`) resolve to
  `.ts`/`.mts`/`.cts` sources; previously any project with
  `moduleResolution: "NodeNext"` silently got `BodyShape::Unknown` for every
  cross-file DTO.

**Phase 11.1 in progress (started 2026-11-10):**
- Milestone 11.1.1 done: `resolve/containment.rs` gates every dependency
  read on canonical-root containment, symlink-free components and a
  regular-file target; `MAX_IMPORT_EDGES`/`MAX_DTO_DECLARATIONS` enforced;
  boundary tests at exact limit and +1 for every budget.
- Milestone 11.1.2 done: run-scoped `ResolverContext` (cache, budgets,
  diagnostics shared per run), single-parse module analysis (dependency
  parsed once; controller seeded from the extractor's parse), Criterion
  suite +4 shared-workload benchmarks; cross-file resolution 40 µs → 30 µs.
- Milestone 11.1.3 done: watch mode dependency invalidation via the
  recorded reverse dependency index; dependent controllers re-analyzed on
  DTO-only changes; conservative merge (failed/uncertain analyses keep
  their edges); e2e + incremental ≡ fresh-sync equivalence tests.
- Milestone 11.1.4 done: `SourceProvider` boundary (disk/index);
  `sync --staged` resolves cross-file DTOs from captured index blobs only;
  staged gate widened to dependency files and tsconfig.json.
- Milestone 11.1.5 done: Zod subset complete (multi-step chains,
  provenance-checked cross-file/forward `z.infer`, namespace
  `createZodDto`, `#src` imports subpaths through both providers).
- Milestone 11.1.6 done: real-binary pipeline acceptance suite
  (`tests/cross_file.rs`) and property coverage (deterministic
  resolution/termination, containment, dependency-edit equivalence);
  engineering record in
  `docs/phases/phase-11.1-resolver-completion.md`.
- Phase 12 done: static global prefix + URI versioning composed in the
  route IR; bounded bootstrap discovery + `bootstrap` config key; exclude
  rules, `@Version` precedence, `VERSION_NEUTRAL`, multi-version variants.
  Record in `docs/phases/phase-12-global-prefix-versioning.md`.
- Phase 13 done: query/param DTO resolution through the shared resolver;
  record in `docs/phases/phase-13-query-param-dtos.md`.
- Next: Phase 13 release finishing (version bump), then the final project
  audit. See `ROADMAP.md` and `PROGRESS.md`.

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
