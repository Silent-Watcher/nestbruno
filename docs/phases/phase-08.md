# Phase 8: Production hardening, CLI UX & release readiness

## Objective

No new features: make the existing implementation reliable enough for real
use and an initial release — reviews, reproducible benchmarks, release
metadata, and a release checklist.

## What was reviewed and what changed

### Benchmarks (new)

`benches/pipeline.rs` (criterion, `cargo bench`) covers the four expensive
stages. Numbers below are from this machine (Linux sandbox, warm cache,
2026-11-06) — read as orders of magnitude, consistent with the ad-hoc
observations in earlier phase documents:

| Benchmark | Input | Median |
| --- | --- | --- |
| `extract` | 100 controllers / 300 routes in one file (~24 KiB) | **~193 µs** |
| `generate` | 100 controllers → 300 requests | **~864 µs** |
| `discover` | 1,000 files (334 controllers, plus `node_modules` that must be skipped) | **~4.0 ms** |
| `sync` no-op | 750-request collection, nothing changed | **~4.6 ms** |

Reading: parsing and generation are linear and fast; a no-op `sync` of a
realistic collection costs a few milliseconds, dominated by reading and
hash-verifying the existing files. No regression found, no optimization
attempted (none justified by measurement). Staged/watch paths are not
benched separately: they reuse these stages and their own overhead (the
linear reconcile pass) is measured in [phase-06.md](phase-06.md).

### Lifecycle end-to-end test (new)

`tests/lifecycle.rs`: one test walks `init` → `check` → `discover` →
`routes` → `sync --dry-run` → `sync` → user edit → `sync --staged
--dry-run` (conflict predicted, nothing written) → `sync --staged` (exit 4,
edit survives) → resolution → in-place update → `generate --force` on an
unchanged file (byte-identical, not rewritten). It also pins the
documented exit-code contract (0 / 1 / 4 in this flow).

### Hardening fixes

- **One debounce constant**: the CLI default now uses
  `WatchOptions::DEFAULT_DEBOUNCE_MS` instead of duplicating `200` in two
  files (audit finding).
- Public API review: left as is — every `pub` item is either the library's
  test/diagnostic surface or consumed by other modules; reducing it would
  be churn without a defect.
- Security review of filesystem operations: no new findings; the existing
  guarantees (containment, symlink refusal, verified writes, deletion
  gates) were re-checked against [security.md](../security.md) and their
  tests remain in place.
- Configuration hardening: no gaps found (`deny_unknown_fields`, relative
  no-`..` paths, size caps, invalid globs are fatal).
- Watch mode under editor behavior: covered by the Phase 6 end-to-end
  suite (atomic saves, bursts, broken files, signal shutdown); no new
  failures.
- Git integration: hardened in Phase 7 (snapshot semantics, conflict
  exclusion, path validation); nothing added.

### Dependency audit

`cargo tree`: 13 direct runtime dependencies (clap, ctrlc, git2, globset,
notify, oxc×4, serde, tempfile, thiserror, toml), each justified in
`Cargo.toml` comments. Duplicate-version check shows only build-time
duplicates introduced by the oxc release train (`fastrand`/`phf` via
`oxc_regular_expression`) — not actionable without forking oxc.
`cargo audit` is not installed in this environment; the release checklist
includes it as a local step, and adding it to CI is recorded in the
roadmap (it needs network access in CI).

### Cross-platform review

- No platform-specific code beyond `cfg(unix)` test gates; paths, writes
  and watching go through `std`, `tempfile` and `notify`.
- Windows: file naming guards reserved names; CI is configured for
  windows/macos but has not been exercised on this machine — the honest
  status stays "CI-configured, untested by the authors".
- Symlink tests and watch end-to-end tests remain Unix-only.

### Documentation accuracy (audit findings fixed)

- `phase-03.md`: "52 tests" → 51 `#[test]` functions + 3 property suites.
- `phase-01.md`: `base_url` pointer note; `init` `--project` target
  semantics.
- `phase-05.md`: sample output now shows the `FAILED` line; the 64 KiB
  text-keep policy for unknown files documented.
- `phase-04.md`: `bruno.json` fixed content (name `"API"`, version `"1"`)
  documented.
- `docs/release.md` (new): the release checklist this phase introduced.
- `Cargo.toml`: `keywords` and `categories` added for publication.
- README unchanged (still what/install/usage/capabilities only).

## Release readiness

- `cargo build --release` passes; the release binary was installed with
  `cargo install --path . --root <tmp>` and smoke-tested (`--version`,
  `init`, `check`, `sync` on a throwaway project).
- Version stays **0.1.0**: it is already published on crates.io, so the
  next release must bump to 0.1.1 (recorded in `STATUS.md` and the
  checklist). No bump was made here because nothing is being published in
  this phase.
- Remaining release blockers: none in code. The pre-publish checklist is
  `docs/release.md`.

## Testing

470 tests before the phase → 471 after (the lifecycle test). fmt, clippy
(`-D warnings`), `cargo test --all-features`, `cargo build --release` and
`cargo bench` all pass.

## Limitations

- Benchmarks measure this sandbox; the numbers are reproducible via
  `cargo bench` but not portable guarantees.
- Windows/macOS remain CI-configured but unverified.
- `cargo audit` not run here (not installed); listed in the checklist.
- No `Cargo.lock` version bump: publishing happens in its own step, by the
  owner, following `docs/release.md`.

## Acceptance criteria

All Phase 8 prompt items are addressed within these limitations: feature
audit (no defects found beyond the listed fixes), CLI/exit-code/diagnostic
review (contract unchanged, now lifecycle-tested), configuration and
filesystem security reviews, watch/Git hardening reviews, benchmarks with
recorded numbers, memory boundedness (existing 1,500-edit session test),
regression suite retained, dependency and unsafe reviews (`unsafe` remains
forbidden crate-wide), public API review, documentation accuracy, release
build + install validation, package metadata, release checklist.

## Commits

- `bench: add pipeline benchmarks`
- `test: add end-to-end lifecycle test`
- `fix: use one constant for the watch debounce default`
- `docs: fix documentation drift and add release metadata`
- `docs: document phase 8 hardening`
