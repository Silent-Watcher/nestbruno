# Development

## Setup

- Rust: stable, edition 2024. Developed and verified with Rust 1.91.1.
  The declared `rust-version` comes from the dependencies' requirements and
  has not been verified against older compilers.
- No other tools are required. Tests run offline and do not read your Git
  configuration or environment.

## Commands

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

All three checks are a hard gate (see `.github/workflows/ci.yml`). Do not
silence lints; fix the code. `unwrap`/`expect` are linted (`clippy::unwrap_used`,
`clippy::expect_used`); tests are exempt, library code must justify any use.
`unsafe` is forbidden crate-wide.

## Tests

- Unit tests live next to the code (`#[cfg(test)]`).
- Integration tests are in `tests/`. They create real temporary projects
  (`tests/common`) and run the actual binary via `CARGO_BIN_EXE_nestbruno`.
  They always pass `-C <tempdir>` and never depend on the working directory.

### Bruno format changes

Generated files are locked by the golden fixtures in `tests/fixtures`. If a
generator change alters them, review the diff as an intentional format change.
To re-verify against Bruno itself, parse the output with the npm package
`@usebruno/lang` (`bruToJson`) and check that `jsonToBru(bruToJson(text))`
equals the text; this was done for phase 4 but is not part of the offline
suite.

### Parser upgrades

`oxc_*` crates must be upgraded together. After any upgrade, re-run the full
suite in a **debug** build (`worst_case_nesting_at_the_size_limit_...` must not
abort) and re-measure per-byte stack use if it fails; see security.md.

### Permission tests and root

Tests that rely on `chmod 000` (unreadable directories) cannot observe
anything when run as root, because root bypasses mode bits; they detect this
and return early. Run the test binaries as an unprivileged user (for example
with `setpriv`) when verifying them in a root-only environment.

## Debugging

`nestbruno --verbose <command>` prints extra diagnostics to stderr, including
symlink notes and, for `discover`, the number of directories listed.
`RUST_BACKTRACE=1` applies as usual to panics (there should be none).

## Profiling

Not yet needed; profiling guidance will be added with Phase 10, which is
driven by measurements.
