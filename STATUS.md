# STATUS.md — current project snapshot

*Last updated: 2026-11-06, after Phase 8.*

## One-paragraph state

NestBruno is a Rust CLI (edition 2024, MSRV 1.91) that statically syncs
NestJS HTTP routes into a Bruno collection. **Phases 1–8 of 10 are
complete** (foundation, discovery, AST route extraction, Bruno generation,
reconciliation sync + dry-run, watch mode with incremental sync, staged Git
synchronization, production hardening with benchmarks). The codebase is
clean: no TODOs, no `unsafe`, no `unwrap` outside tests, docs in `docs/`
match the implementation. A VitePress documentation site lives in
`website/` (guide through Phase 8; `npm run build` green). Release
readiness: gates green, release build + install smoke passed,
`docs/release.md` is the pre-publish checklist. Next phase is **Phase 9:
advanced route semantics & request metadata**.

## Build & test status (verified 2026-11-06, toolchain stable 1.99.0)

- `cargo fmt --check` — pass
- `cargo clippy --all-targets --all-features -- -D warnings` — pass
- `cargo test --all-features` — pass, 471 tests
- `cargo build --release` + `cargo install --path .` smoke — pass
- `cargo bench` — reproducible numbers in `docs/phases/phase-08.md`
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
| `sync --staged [--dry-run]` | same reconciliation against the Git index state; no-op unless a relevant file is staged; never touches Git |
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

0. **Documentation site** (`website/`): VitePress with local search,
   animated SVG logo (respects `prefers-reduced-motion`), landing page and
   the guide through Phase 7. Commands: `npm run dev` / `npm run build` /
   `npm run preview` inside `website/`. Content mirrors `docs/`; keep both
   consistent when behavior changes.
1. **Phase 9 — Advanced route semantics & request metadata** (richer
   parameter/type metadata, still static; must not destabilize route
   identity). See [ROADMAP.md](ROADMAP.md) and the Phase 9 prompt.
2. Publishing: v0.1.0 is taken on crates.io — bump to 0.1.1 and follow
   [docs/release.md](docs/release.md).

## Decisions an agent must not silently reverse

See [DECISIONS.md](DECISIONS.md). Rules of engagement: [AGENTS.md](AGENTS.md).
