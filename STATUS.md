# STATUS.md — current project snapshot

*Last updated: 2026-11-06, after the Phase 1–6 audit and quality-gate run.*

## One-paragraph state

NestBruno is a Rust CLI (edition 2024, MSRV 1.91) that statically syncs
NestJS HTTP routes into a Bruno collection. **Phases 1–6 of 10 are
complete** (foundation, discovery, AST route extraction, Bruno generation,
reconciliation sync + dry-run, watch mode with incremental sync). The
codebase is clean: no TODOs, no `unsafe`, no `unwrap` outside tests, docs in
`docs/` match the implementation closely. Next phase is **Phase 7: Git
staged synchronization** (`sync --staged`).

## Build & test status (verified 2026-11-06, toolchain stable 1.99.0)

- `cargo fmt --check` — pass
- `cargo clippy --all-targets --all-features -- -D warnings` — pass
  (after fixing 2 new-lint hits in `src/config.rs`; see KNOWN_ISSUES)
- `cargo test --all-features` — pass, 443 tests (317 lib + 126 integration)
- Sandbox note: rustup proxies fail with `unknown proxy name: 'ZCode-…'`;
  prepend `~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin` to PATH
  (recorded in [AGENTS.md](AGENTS.md)).

## What works (all commands)

| Command | Notes |
| --- | --- |
| `init` | writes `nestbruno.toml`, never overwrites |
| `check` | validates config + source dir |
| `discover` | lists candidate controller files |
| `routes` | prints routes/params/locations |
| `generate [--force]` | writes `.bru` requests; never deletes; overwrite only with `--force` |
| `sync [--dry-run]` | create/update-in-place/delete-unmodified-generated; conflicts preserved & reported (exit 4) |
| `watch [--debounce MS]` | incremental sync on change; clean shutdown; fallbacks to full resync |

Details: [docs/project-status.md](docs/project-status.md).

## Health

- Known runtime bugs: none known ("Known issues: None" in project-status;
  audit found no TODO/FIXME/panic paths in production code).
- Property tests are live (saved regressions in `proptest-regressions/`).
- Documentation drift exists but is minor — tracked in
  [KNOWN_ISSUES.md](KNOWN_ISSUES.md).

## Main limitations (user-visible)

Name-based decorator matching; top-level classes only; no global prefix /
versioning / `RouterModule`; 256 KiB source cap; no field-level merge (a user
edit locks the whole request); nothing deleted while analysis incomplete;
`sync` re-reads all `.bru` per run; watch does not watch the collection and
does one linear reconcile pass per batch. Full list:
[docs/project-status.md](docs/project-status.md).

## Next steps

1. **Phase 7 — Git staged synchronization** (`sync --staged`). Starting
   point: the analyzer currently reads only filesystem files
   (`nest/mod.rs::analyze_files`); the prompts call for a `SourceInput`
   (filesystem | in-memory) abstraction so staged content can be parsed
   without the parser learning about Git. Phase-06 doc notes this hook:
   "the analyzer would need an in-memory input abstraction".
   Requirements: see [ROADMAP.md](ROADMAP.md) and the Phase 7 prompt.
2. After that: Phase 8 (hardening/release), 9 (richer metadata), 10
   (OpenAPI).

## Decisions an agent must not silently reverse

See [DECISIONS.md](DECISIONS.md). Rules of engagement: [AGENTS.md](AGENTS.md).
