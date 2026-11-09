# Roadmap

Implemented phases are listed in [project-status.md](project-status.md), and
the agent-facing phase state is tracked in the repository root `PROGRESS.md`.
Phase 10 and Phase 11 are independent items; Phase 11 does not replace or
renumber the OpenAPI work.

The original conceptual phase sequence was reorganized: dry run shipped in
Phase 5, while Watch Mode and incremental synchronization were merged into
Phase 6.

| Phase | Title | Notes |
| --- | --- | --- |
| 10 | OpenAPI interoperability | **Not started** — export/validate from the route IR; never reparse TypeScript |
| 11 | Cross-file DTO resolution (class + Zod) | **Partial / unverified** — on-demand local import resolution; see [Phase 11 handoff](phases/phase-11-cross-file-dto-resolution.md) |

Phase 11 is not a released capability. Its source-level findings, owner-approved
constraints, implementation sequence, and completion criteria are documented
in [phases/phase-11-cross-file-dto-resolution.md](phases/phase-11-cross-file-dto-resolution.md).

Candidates raised by measurements but not scheduled: restricting the Watch
Mode reconciliation pass to affected routes (see
[phases/phase-06.md](phases/phase-06.md), Performance).
