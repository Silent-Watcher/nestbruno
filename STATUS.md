# STATUS.md — current project snapshot

*Last updated: 2026-11-06, after Phase 7.*

## One-paragraph state

NestBruno is a Rust CLI (edition 2024, MSRV 1.91) that statically syncs
NestJS HTTP routes into a Bruno collection. **Phases 1–7 of 10 are
complete** (foundation, discovery, AST route extraction, Bruno generation,
reconciliation sync + dry-run, watch mode with incremental sync, staged Git
synchronization). The codebase is clean: no TODOs, no `unsafe`, no `unwrap`
outside tests, docs in `docs/` match the implementation closely. A VitePress
documentation site lives in `website/` (landing page, animated logo, local
search, guide for Phases 1–7; `npm run build` is green). Next phase is
**Phase 8: production hardening / release readiness**.

## Build & test status (verified 2026-11-06, toolchain stable 1.99.0)

- `cargo fmt --check` — pass
- `cargo clippy --all-targets --all-features -- -D warnings` — pass
- `cargo test --all-features` — pass, 470 tests (330 lib + 140 integration)
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
1. **Phase 8 — Production hardening, CLI UX & release readiness**. No new
   features: exit-code contract, configuration validation, filesystem
   security audit, watch/Git hardening, benchmarks (none exist yet),
   cross-platform review, dependency audit, release checklist. See
   [ROADMAP.md](ROADMAP.md) and the Phase 8 prompt.

## Decisions an agent must not silently reverse

See [DECISIONS.md](DECISIONS.md). Rules of engagement: [AGENTS.md](AGENTS.md).
