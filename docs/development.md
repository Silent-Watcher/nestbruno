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

## Debugging

`nestbruno --verbose <command>` prints extra diagnostics to stderr.
`RUST_BACKTRACE=1` applies as usual to panics (there should be none).

## Profiling

Not yet needed; profiling guidance will be added with Phase 10, which is
driven by measurements.
