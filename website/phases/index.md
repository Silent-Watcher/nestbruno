# Development Phases

NestBruno is built phase by phase; each phase is complete (implementation,
tests, lint, documentation, commits) before the next starts. The full
engineering record — including design decisions, measured numbers, found
bugs and mutation checks — lives in the repository under `docs/phases/`.

| Phase | Title | Status | What it gave the user |
| --- | --- | --- | --- |
| 1 | Project foundation | ✅ | CLI skeleton, `nestbruno.toml`, error/exit-code model, test infrastructure |
| 2 | Controller discovery | ✅ | `discover` — fast, deterministic candidate finding from paths alone |
| 3 | Route extraction | ✅ | `routes` — static AST analysis into a stable route IR |
| 4 | Bruno generation | ✅ | `generate` — deterministic `.bru` files with safe naming and writes |
| 5 | Synchronization | ✅ | `sync`, `sync --dry-run` — ownership stamps, conflicts, safe deletion |
| 6 | Watch mode | ✅ | `watch` — incremental live sync with debounced events and fallbacks |
| 7 | Git staged synchronization | planned | `sync --staged` |
| 8 | Production hardening | planned | benchmarks, cross-platform, release readiness |
| 9 | Richer route metadata | planned | richer parameters/types, still static |
| 10 | OpenAPI interoperability | planned | export/validate from the same route IR |

Pages: [Phase 1](/phases/phase-01) · [Phase 2](/phases/phase-02) ·
[Phase 3](/phases/phase-03) · [Phase 4](/phases/phase-04) ·
[Phase 5](/phases/phase-05) · [Phase 6](/phases/phase-06)
