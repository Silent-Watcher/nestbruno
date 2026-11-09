# KNOWN_ISSUES.md — verified problems and drift

From the 2026-11-06 audit (commit `48b201f`). "Drift" = documentation or
tooling mismatch, not a runtime bug. Runtime known issues: none known.

## Fixed during the audit

- **Clippy drift (fixed 2026-11-06).** Toolchain stable 1.99.0 (newer than
  the project's lint runs) added `needless_borrows_for_generic_args` hits in
  `src/config.rs:132-133` (`map_err(&invalid)`). Fixed by dropping the `&`;
  gates green again. Expect more lint drift when toolchains bump — do not
  disable lints, fix the code.

## Documentation drift (fixed in the Phase 8 pass, 2026-11-06)

1. ~~`docs/phases/phase-03.md` claims "52 tests"~~ — corrected to 51 `#[test]`
   fns + 3 property suites.
2. ~~`docs/phases/phase-01.md` omits `base_url`~~ — pointer note added.
3. ~~`docs/architecture.md` "Planned components" stale~~ — fixed earlier.
4. ~~`docs/phases/phase-05.md` sample omits the `FAILED` line~~ — added.
5. ~~Undocumented small behaviors~~ — collection name `"API"`, the 64 KiB
   text-keep policy, `init`'s `--project` target semantics documented; the
   duplicated debounce default removed from the code (single
   `WatchOptions::DEFAULT_DEBOUNCE_MS`).

## Claims not reproducible from the repository

These are documented, plausible, but unverifiable in-tree; treat as
assertions, not guarantees, and re-verify before relying on them:

- Phase-04's "output verified against Bruno's own parser (`@usebruno/lang`
  0.40.0, byte-exact round-trip)" — verification script intentionally not
  committed; in-tree evidence is only golden fixtures.
- All performance numbers in phase docs (discovery 100k files ≈ 30 ms,
  watch reconcile timings, etc.) — ad-hoc sandbox runs, no benchmark suite.
- Phase-02 permission tests "run as unprivileged user" and phase-06 "watch
  suite run 60× consecutively" — one-off manual runs.

## Real gaps (accepted, tracked)

- ~~**No `LICENSE`**~~ — resolved 2026-11-06: MIT chosen by the owner,
  `LICENSE` added, `license = "MIT"` in Cargo.toml,
  `cargo publish --dry-run` passes.
- ~~**No benchmark suite**~~ — resolved in Phase 8: `benches/pipeline.rs`
  (Criterion) with a cross-file resolution benchmark added 2026-11-09.
- **Watch e2e tests Unix-only**; CI matrix (linux/macos/windows) exercised
  on Linux only.
- **Check-then-write races** in sync/watch (documented limitation in
  phase-05; mitigated by re-verification, not eliminated).
- **FNV-1a fingerprints are not collision-resistant** (documented;
  acceptable because stamps are verified by content hash, not by trust).
- **`sync` re-reads every `.bru` per run**; watch does not watch the
  collection (user edits made in Bruno between events are caught by
  write-time verification, not by watching).
- Known unsupported constructs (documented, not bugs): name-based decorator
  matching, top-level classes only, no global prefix/versioning/
  `RouterModule`, 256 KiB source cap, no field-level merge, `@All`/array
  paths/hostile paths not generated.

## Environment (this machine, not the project)

- rustup proxy binaries fail under the ZCode sandbox
  (`unknown proxy name: 'ZCode-…'`) because the sandbox rewrites
  `argv[0]`. Use the toolchain binaries directly
  (`PATH=$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH`).
  Not a project bug; recorded so agents don't misdiagnose it.


## Phase 11 — resolution state (updated 2026-11-09, verified)

The 2026-11-08 blocker list below was based on an unverified working tree.
Phase 11 was then completed and verified (535 tests, fmt + clippy
`-D warnings` clean) and shipped in v0.2.0. Disposition of the former
blockers:

Resolved and verified:

- ~~Uncertain Zod output~~ — `DtoResolution::Uncertain` now keeps
  `BodyShape::Unknown` (`src/nest/extract.rs`), pinned by extraction tests.
- ~~Re-export semantics~~ — aliased re-exports map public names to source
  names; declarations resolve only through real exported bindings
  (`src/resolve/mod.rs::lookup_export`), covered by unit tests.
- ~~Cycle detection~~ — explicit `(module, symbol)` resolution stack;
  circular imports yield `Unknown` + warning (tested).
- ~~Bare-name `createZodDto` fallback~~ — removed; a real `nestjs-zod`
  import is required (`does_not_recognize_unimported_create_zod_dto`).
- ~~Benchmark call sites~~ — updated and executed (`cargo bench` run
  2026-11-09; cross-file resolution benchmark added: ~40 µs median).

Still open (deferred; the accepted limitations of the shipped feature):

- **Import path containment is lexical only.** `resolve/module.rs` checks
  for `..` escapes and existence but does not canonicalize or reject
  symlinks; an imported symlink could point outside the project. Nothing
  is ever executed regardless; hardening is tracked as a follow-up.
- **Duplicate parsing.** The controller is parsed by `extract.rs` and again
  by the resolver for imports; each dependency module is parsed twice
  (module analysis + Zod analysis). A `ModuleAnalysis` source clone is
  retained without a consumer.
- **Cache scope too narrow.** A fresh `DtoResolver` is built per route, so
  shared dependency files are re-read/re-parsed across routes and
  Controllers within one run.
- **Watch dependency changes not wired.** `Workspace::refresh()` only
  schedules Matcher-relevant (controller-candidate) files; editing an
  imported DTO file does not invalidate dependent Controllers. A reverse
  dependency index or safe full-resync fallback is required.
- **Staged dependencies not wired.** `sync --staged` analyzes controller
  blobs from the index without project context, so cross-file resolution
  is disabled there (same-file DTOs only). This is the safe behavior until
  an index-only source provider exists.
- **Graph budgets partly declarative.** `MAX_IMPORT_EDGES` (4,096) is
  declared but not enforced; `MAX_DTO_DECLARATIONS` (1,024) emits a
  warning without stopping extraction. `MAX_REEXPORT_HOPS` (4),
  `MAX_DEPENDENCY_FILES` (512) and `MAX_TOTAL_DEPENDENCY_BYTES` (4 MiB)
  are enforced.
- **Zod analyzer gaps.** Multi-step transformation chains are not
  recognized (only a single transformation applied to a base schema
  identifier); `z.infer` resolves only against same-file const schemas
  declared earlier; namespace-imported `createZodDto` is not recognized.
- **Test coverage is not acceptance coverage.** Resolver unit tests exist;
  comprehensive `.bru` integration tests, reverse-dependency watch tests,
  staged index/worktree divergence tests, and property tests for the
  resolver are still missing.
