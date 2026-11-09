# Development Phases

NestBruno is built phase by phase. A phase is considered complete only after
its implementation, tests, documentation, and quality gates have been
verified. The full engineering record lives in the repository under
`docs/phases/`.

| Phase | Title | Status | What it gives the user |
| --- | --- | --- | --- |
| 1 | Project foundation | ✅ | CLI, configuration, error model, test infrastructure |
| 2 | Controller discovery | ✅ | Fast, deterministic candidate discovery from paths alone |
| 3 | Route extraction | ✅ | Static AST analysis into a stable route IR |
| 4 | Bruno generation | ✅ | Deterministic `.bru` files with safe naming and writes |
| 5 | Synchronization | ✅ | Ownership stamps, conflicts, safe deletion and dry-run |
| 6 | Watch mode | ✅ | Incremental live sync with debounced events and fallbacks |
| 7 | Git staged synchronization | ✅ | Reconcile the state represented by the Git index |
| 8 | Production hardening | ✅ | Criterion benchmarks, lifecycle coverage, release checklist |
| 9 | Route semantics & request metadata | ✅ | Parameter types and same-file class/interface DTO body skeletons |
| 10 | OpenAPI interoperability | Planned | Export/validate from the existing route IR |
| 11 | Cross-file DTO resolution (class + Zod) | In progress, unverified | Resolve imported DTO/schema definitions without executing application code |

Phase 11 is under development and is not yet a released capability. Its
approved architecture and current blockers are tracked in the repository's
[Phase 11 status page](/phases/phase-11).

Pages: [Phase 1](/phases/phase-01) · [Phase 2](/phases/phase-02) ·
[Phase 3](/phases/phase-03) · [Phase 4](/phases/phase-04) ·
[Phase 5](/phases/phase-05) · [Phase 6](/phases/phase-06) ·
[Phase 7](/phases/phase-07)
