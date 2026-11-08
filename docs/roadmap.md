# Roadmap

Future work only. Implemented phases are listed in
[project-status.md](project-status.md).

The list follows the phase prompts provided so far (the original conceptual
plan in the first prompt was reorganized: dry run arrived in phase 5, and
watch mode and incremental synchronization were merged into phase 6).

| Phase | Title | Notes |
| --- | --- | --- |
| 7 | ~~Git staged files and selective synchronization~~ | **done** — `sync --staged`; see [phases/phase-07.md](phases/phase-07.md) |
| 8 | Production hardening, CLI UX and release readiness | exit-code contract, configuration validation, benchmarks, cross-platform review, release checklist |
| 9 | Advanced route semantics and request metadata | **done** — parameter types, DTO fields, `BodyShape`, in-place updates; see [phases/phase-09.md](phases/phase-09.md) |
| 10 | OpenAPI interoperability | export/validate from the route IR |

Candidates raised by measurements but not scheduled: restricting the
reconciliation pass to the affected routes in watch mode (see
[phases/phase-06.md](phases/phase-06.md), Performance).

The authoritative prompts for each phase are provided separately; each phase
document in `phases/` records the decisions the next phase builds on.
