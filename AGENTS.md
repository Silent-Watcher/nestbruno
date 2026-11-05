# AGENTS.md — rules for AI agents working on NestBruno

NestBruno is a Rust CLI that statically analyzes a NestJS project and keeps a
Bruno API collection synchronized with its HTTP routes. The project is
developed in phases; this file tells an agent how to work here without
repeating past mistakes or past investigation.

## Read before any work

1. `STATUS.md` — current snapshot.
2. `PROGRESS.md` — per-phase implementation state.
3. Relevant section of `ARCHITECTURE.md`.
4. `ROADMAP.md` — what comes next.
5. `KNOWN_ISSUES.md` — verified problems and documentation drift.
6. `DECISIONS.md` — decisions you must not silently reverse.
7. The authoritative prompt for the phase you are implementing (kept outside
   the repository; the plan is summarized in `ROADMAP.md`).
8. The phase documents in `docs/phases/` for everything already built.

Do not re-audit the whole repository each session; the context files are the
source of truth. If you find they are wrong, fix the file, not just the code.

## Hard rules

- **Phases only.** Implement exactly the requested phase. Never start the
  next phase, never pull future-phase functionality in early.
- **Quality gates before every commit**, all must pass:
  ```sh
  cargo fmt --check
  cargo clippy --all-targets --all-features -- -D warnings
  cargo test --all-features
  ```
  Never disable a lint to make a gate pass; fix the code.
- **No `unsafe`, no `unwrap()`/`expect()` in non-test code.** Enforced by
  `unsafe_code = "forbid"` and clippy (`unwrap_used`/`expect_used` warn;
  tests are exempt via `clippy.toml`).
- **Static analysis only.** Never execute TypeScript/JavaScript, never boot
  NestJS, never shell out to Node, never call the network. This is a security
  requirement, not a style preference.
- **Never destroy user data.** Unknown or modified `.bru` files are never
  silently overwritten or deleted; conflicts are reported. Do not weaken the
  Phase 5 ownership/reconciliation rules for convenience.
- **Determinism.** Same input ⇒ byte-identical output. No timestamps, random
  IDs, or filesystem-order dependence in generated files.
- **README.md stays small** (what it is, install, usage, capabilities).
  Everything else goes in `docs/`.
- **Update context files after meaningful work**: `PROGRESS.md`, `STATUS.md`,
  and the phase doc in `docs/phases/` for the phase you touched.
- **Commits**: small, atomic, conventional (`feat:`, `fix:`, `test:`,
  `docs:`, `chore:`). Inspect the full diff before committing; commit only
  intended files; verify `git status` is clean afterwards.

## Environment notes

- Building requires Rust (edition 2024, `rust-version = "1.91"` because oxc
  0.116 pins it). If `cargo` prints
  `error: unknown proxy name: 'ZCode-...'`, the rustup proxy is being
  invoked with a rewritten `argv[0]` by the sandbox; work around it with:
  ```sh
  export PATH="$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
  ```
- The watch-mode end-to-end tests (`tests/watch.rs`) are Unix-only.

## Architecture invariants to preserve

- Library crate holds all behavior; `main.rs` only wires the CLI.
- Layer boundary: discovery → oxc parser → route IR → Bruno model →
  serializer → writer. The parser knows nothing about Bruno; the generator
  never reads source or `.bru` files; `bruno/writer.rs` is the only writer.
- Diagnostics: a `Severity::Error` diagnostic means "this path's result is
  *unknown*, not empty". Deletion and destructive sync are gated on
  `is_incomplete()` because of this contract.
- Reconciliation plans are pure; application re-verifies every hash and path
  before writing.
