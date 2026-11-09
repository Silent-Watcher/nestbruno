# Phase 13 — Query DTO and Param DTO resolution

Owner-authorized (2026-11-10). The cross-file resolver (same context,
cache, limits, source providers — no duplication) now also resolves
unnamed `@Query()` and `@Param()` object parameters.

## Design

- `RouteParameter.fields` carries the resolved property names of an
  unnamed object parameter. Same-file DTOs fill it during extraction;
  cross-file resolution fills it in the same pass that resolves `@Body`
  (extended filter: body parameters plus unnamed query/param objects).
- **Query**: resolved fields become *disabled, empty* `params:query`
  entries — never sent, never invented values — deduplicated against
  named `@Query('x')` parameters. Unresolved objects keep the existing
  "fields not inferred" note.
- **Param**: path parameters still come from the URL (the source of
  truth). DTO fields are matched against them; fields not present in the
  URL are documented in the request (`DTO fields not present in the URL
  path: ...`) — placeholders are never invented. NestJS semantics
  preserved: `@Param() dto` (an object) and `@Param('id') id` (one named
  parameter) remain distinct.
- Resolution results are keyed per parameter index and applied by kind:
  body fields become the body shape, query/param fields land on the
  parameter. No field-level merge, no runtime validation inference.

## Verification

- 3 end-to-end tests (`tests/cross_file.rs`): cross-file query DTO to
  disabled query parameters; named + DTO deduplication; param DTO matched
  against URL parameters with an extra DTO field documented and no
  invented placeholder.
- Full suite: 628 tests; fmt and clippy `-D warnings` clean.

## Known limits (accepted)

- Nested DTO properties are not flattened; only top-level property names.
- A param DTO is metadata only: it can document, never alter, the URL.
