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
- **No benchmark suite** — planned for Phase 8.
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


## Phase 11 — current unverified implementation blockers (2026-11-08)

These are source-inspection findings from the supplied, uncommitted working
tree. They are not represented as passing/failing test results because the
Rust toolchain was unavailable during this handoff.

- **Uncertain Zod output — fix pending verification:** the inspected version
  mapped `DtoResolution::Uncertain` to `BodyShape::Fields`. The latest handoff
  changes this branch to preserve `BodyShape::Unknown`; an extractor-level
  regression and compile/test run are still required before closing the issue.
- **Import path containment is incomplete:** `resolve/module.rs` performs
  lexical checks and `exists()` checks but no canonical symlink containment or
  regular-file validation. Absolute/re-export path representation is also
  inconsistent. Treat as a security blocker before enabling the resolver.
- **Re-export semantics are incomplete:** aliased re-export names are mapped
  incorrectly in lookup, and local declarations are considered even without
  confirming that they are exported. Verify Oxc AST field semantics and map
  public names to source-local names.
- **Duplicate parsing:** the Controller is parsed once by `extract.rs` and
  again by the resolver to discover imports; Zod analysis creates a second Oxc
  parser pass for a module. A `ModuleAnalysis.source` clone is also retained
  without a current consumer.
- **Cache scope too narrow:** the resolver is created by
  `resolve_cross_file_dtos()` at each route call site, so shared dependencies
  are not reused across a Controller analysis, much less across Controllers
  in one run.
- **Watch dependency changes not wired:** `Workspace::refresh()` uses
  `Matcher::is_relevant()`, which intentionally matches Controller candidates;
  ordinary DTO/schema files are ignored. A reverse dependency index or safe
  full-resync fallback is required.
- **Staged dependencies not wired:** `analyze_sources()` currently invokes
  extraction without project/source context, and `staged::collect_sources()`
  only reads Matcher-relevant Controller entries. The resolver must read each
  imported dependency from the Git index snapshot on demand, with no working-
  tree fallback.
- **Graph budgets are partly declarations only:** `MAX_IMPORT_EDGES` is not
  enforced; `MAX_DTO_DECLARATIONS` emits a warning but does not stop/cap
  extraction. Explicit `(module, symbol)` cycle state is absent.
- **Zod analyzer gaps:** `analyze_chained()` has no call site; `z.infer` must be
  tied to an imported Zod binding; exported visibility is not consistently
  validated. The unverified handoff removes the bare-name `createZodDto`
  fallback and adds a focused regression test, but this has not been executed.
- **Test coverage is not acceptance coverage:** the uncommitted resolver has
  unit tests, but comprehensive `.bru` integration, reverse-dependency Watch,
  staged index/worktree divergence, property tests, and resolver benchmarks
  have not been verified.
- **Benchmark call sites updated:** the three existing `extract_controllers()`
  calls in `benches/pipeline.rs` now pass an explicit empty dependency context
  for the expanded extractor signature. This compatibility edit is uncompiled.
- **Debug output removed:** an `eprintln!` in import/re-export extraction was
  removed during this handoff to avoid leaking debug output into normal CLI
  execution. This one-line cleanup has not been compiled in this environment.

The authoritative resolution steps and acceptance checklist are in
[`docs/phases/phase-11-cross-file-dto-resolution.md`](docs/phases/phase-11-cross-file-dto-resolution.md).
