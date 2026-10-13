# Roadmap

Future work only. Nothing here is implemented; see
[project-status.md](project-status.md) for what exists.

The list follows the phase prompts provided so far (the original conceptual
plan in the first prompt was reorganized: dry run arrived in phase 5, and
watch mode and incremental synchronization were merged into phase 6).

| Phase | Title | Notes |
| --- | --- | --- |
| 7 | Git staged files and selective synchronization | `sync --staged`; needs a source abstraction so the analyzer can read staged content instead of files on disk |
| 8 | Production hardening, CLI UX and release readiness | exit-code contract, configuration validation, benchmarks, cross-platform review, release checklist |
| 9 | Advanced route semantics and request metadata | richer parameter and type metadata, still static |
| 10 | OpenAPI interoperability | export/validate from the route IR |

Candidates raised by measurements but not scheduled: restricting the
reconciliation pass to the affected routes in watch mode (see
[phases/phase-06.md](phases/phase-06.md), Performance).

The authoritative prompts for each phase are provided separately; each phase
document in `phases/` records the decisions the next phase builds on.
