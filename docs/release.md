# Release checklist

Practical gates for cutting a NestBruno release. Everything here is runnable
from a clean checkout; do not skip a line because the previous release
passed it.

## Code quality

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-features` (includes the property suites and the
      Unix-only watch end-to-end tests)
- [ ] `cargo bench` runs; times compared against the previous recorded
      numbers (`docs/phases/phase-08.md`); regressions explained
- [ ] No new clippy lints disabled to make the gates pass

## Release build

- [ ] `cargo build --release`
- [ ] The release binary produces the same output as the debug build on the
      integration fixtures (run `cargo test --release` once)

## Installation

- [ ] `cargo install --path .` succeeds from a clean checkout
- [ ] `nestbruno --version` works; `init` → `check` → `sync` pass on a
      throwaway NestJS-shaped project

## Package

- [ ] `cargo package` lists only intended files (`cargo package --list`)
- [ ] `license`, `description`, `repository`, `readme` are current
- [ ] Version bumped **only** for this release (crates.io versions are
      immutable; `cargo publish` refuses duplicates)
- [ ] `cargo publish --dry-run` passes

## Dependencies

- [ ] `cargo tree` reviewed: no new duplicate/dormant dependency
- [ ] `cargo audit` run if available (not part of CI)

## Documentation

- [ ] `docs/project-status.md` reflects the released state (commands table,
      limitations)
- [ ] `docs/phases/phase-XX.md` exists for everything included
- [ ] `STATUS.md` / `PROGRESS.md` updated; the VitePress site
      (`website/`, `npm run build`) builds and mirrors the released
      behavior
- [ ] `README.md` still limited to: what it is, install, usage,
      capabilities

## Git

- [ ] Working tree clean; the release is one focused commit/ tag
- [ ] `git status` clean after tagging
