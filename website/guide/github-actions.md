# CI with GitHub Actions

`nestbruno verify` exits non-zero when the Bruno collection no longer
matches the routes, so a Pull Request that changes controllers without
updating the collection cannot slip through. This page is the maintained
workflow template — verification only, never automatic synchronization.

## The workflow

Copy this to `.github/workflows/nestbruno.yml` in the repository that holds
your NestJS project and its Bruno collection:

```yaml
name: Verify Bruno Collection

on:
  pull_request:

permissions:
  contents: read

jobs:
  verify:
    runs-on: ubuntu-latest

    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install NestBruno
        run: cargo install nestbruno --locked

      - name: Verify Bruno collection
        run: nestbruno verify
```

## How it behaves

- The job runs on every Pull Request and fails when `nestbruno verify`
  exits with code **1** (the collection is out of sync: missing, outdated
  or stale requests) or **4** (conflicts, or the analysis could not
  establish a complete state). Exit **0** passes.
- `nestbruno verify` is strictly read-only: the workflow cannot modify your
  repository, and it never runs `nestbruno sync` in CI. Regenerating the
  collection is a deliberate local step (`nestbruno sync`, then commit).
- `permissions: contents: read` is the minimum the job needs; no secrets
  are required.
- NestBruno is installed from its published [crates.io](https://crates.io/crates/nestbruno)
  package; `verify` requires NestBruno 0.5.0 or later. `--locked` builds
  against the published `Cargo.lock`. The crate builds on stable Rust
  (MSRV 1.91 — if you need an exact toolchain, use
  `dtolnay/rust-toolchain@1.91.0` instead of `@stable`).

## Using the JSON report in later steps

`verify --format json` prints exactly one JSON document describing the
drift (`status`, `summary`, `changes`, `conflicts`, `diagnostics`), which
you can feed to subsequent automation steps:

```yaml
      - name: Verify Bruno collection
        run: nestbruno verify --format json > nestbruno-report.json

      - name: List pending changes
        if: failure()
        run: |
          jq -r '.changes.created[], .changes.updated[], .changes.deleted[]' nestbruno-report.json
          jq -r '.conflicts[] | "CONFLICT \(.path): \(.description)"' nestbruno-report.json
```

Keep the report file out of version control (or write it to
`${{ runner.temp }}`).

## Limitations

- `cargo install nestbruno --locked` compiles the crate from source on
  every run (roughly a minute on hosted runners). GitHub Actions caching
  (`Swatinem/rust-cache`) targets workspace builds and does not cache
  `cargo install`, so the template ships without caching. Pin the action
  versions your repository trusts.
- A workflow is only as trusted as its inputs: this template runs no
  scripts of the repository itself and needs no secrets, but it does
  build a published crate — pin the toolchain action if your threat model
  requires it.
- This is a workflow template, not a GitHub Marketplace Action; NestBruno
  does not publish or maintain a custom action runtime.
- The example has been validated for YAML syntax and against the current
  CLI (`verify`, `verify --format json`), but it is not executed inside
  this repository's CI — adapt paths and triggers to your repository.
