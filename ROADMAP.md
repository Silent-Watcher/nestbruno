# ROADMAP.md — remaining phases (9–10)

Based on the authoritative phase prompts (kept outside the repo) and the
actual state in [PROGRESS.md](PROGRESS.md). Phases 1–8 are complete; nothing
below is implemented. Each phase starts only on explicit instruction, ends
with focused commit(s), and must pass fmt/clippy/test gates.

## Phase 7 — Git staged synchronization — DONE (2026-11-06)

`sync --staged [--dry-run]` shipped as designed. Record:
`docs/phases/phase-07.md`.

## Phase 8 — Production hardening — DONE (2026-11-06)

Benchmarks (`cargo bench`, `docs/phases/phase-08.md`), lifecycle e2e test,
audit doc-drift fixed, release checklist (`docs/release.md`), package
metadata, release build + install smoke. Remaining (tracked): `cargo audit`
in CI (needs network), Windows/macOS exercised by the authors.

## Phase 9 — Advanced route semantics & request metadata — DONE (2026-11-08)

Richer Bruno requests from static information only. Record:
`docs/phases/phase-09.md`.

- `RouteParameter.type_name` (keyword or bare identifier only) and
  `BodyShape` (`None`/`Unknown`/`Fields`) on `Route.body`.
- Same-file class/interface DTO property extraction (max 64, source order).
- Generator: DTO fields → JSON skeleton (`null` per property); named
  `@Body('field')` merged in; type metadata in `docs`.
- Identity unchanged: metadata changes update in place (sync integration
  tests). Backward compatible with phases 4–6.
- 495 tests, fmt + clippy green, golden fixtures regenerated.

## Phase 10 — OpenAPI interoperability

**Objective:** export the existing route IR to OpenAPI (prefer 3.1); optional
`openapi validate --spec <file>` comparison; consume the IR, never reparse.
- Deterministic document (stable ordering, no timestamps), config for
  title/version/description/servers; `:id` → `{id}` reusing existing
  normalization; operationIds from controller+handler+method+path with
  deterministic collision handling; conservative parameters/schemas/
  responses (default response when nothing is known); no network, no `$ref`
  fetching, no execution.
- CLI per existing conventions; YAML and/or JSON output; golden tests;
  security tests (traversal, malicious specs); must not regress Bruno
  generation (same IR feeds both).

## Cross-phase notes

- Phase 6 doc lists a measurement-motivated candidate (restrict watch
  reconciliation to affected routes) that is **not scheduled**; revisit only
  with Phase 8 benchmark evidence.
- `docs/roadmap.md` (user-facing) mirrors this list in short form; keep the
  two consistent when phases complete.
