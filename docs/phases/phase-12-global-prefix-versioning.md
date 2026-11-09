# Phase 12 — Global prefix and URI versioning

Owner-authorized (2026-11-10). Effective route URLs now include the
statically analyzable NestJS application configuration: `setGlobalPrefix`
and `VersioningType.URI` versioning. Composition happens in the route IR —
never as string replacement in the generator.

## Design

- **Bootstrap discovery is bounded**: the optional `bootstrap` key in
  `nestbruno.toml` wins; otherwise the first conventional candidate
  (`src/main.ts`, then `main.ts`) is used. Never a repository scan. A
  missing conventional file is normal (no prefix, no versioning); a
  configured-but-missing or unreadable file warns once.
- **`app.setGlobalPrefix('api', { exclude: [...] })`** (`src/nest/app.rs`):
  static string prefixes only. Exclude rules support exact paths and
  trailing-`*` wildcards, as plain strings or `{ path, method }` objects —
  request methods are statically matchable per route. Any unmatchable
  pattern (leading `*`, `:param`, `?`, dynamic expressions) disables the
  prefix entirely with a warning: an honest missing prefix beats a wrong
  URL. Excludes match the path WITHOUT the prefix and version (NestJS
  semantics).
- **`app.enableVersioning({ type: VersioningType.URI, defaultVersion })`**:
  `VersioningType` and `VERSION_NEUTRAL` are provenance-checked against a
  real `@nestjs/common` import. `defaultVersion` may be a string, an array,
  or `VERSION_NEUTRAL`. HEADER/MEDIA_TYPE strategies are identified and
  reported but never rendered as URL segments (they do not appear in
  URLs); an unrecognizable `type` warns and applies nothing.
- **Version precedence** (NestJS): method-level `@Version(...)` overrides
  the controller-level `version` (`@Controller({ path, version })`),
  which overrides the application default. `VERSION_NEUTRAL` at any level
  removes the segment. Multiple declared versions produce one route
  variant per version — the generator emits one request each (collision-
  suffixed names), and each variant has its own identity.
- **Composition**: `/{prefix}/v{n}/{controller}/{method}` in the extractor
  (`routes()`), with prefix and version as first-class fragments. The
  existing `compose_path` normalization handles slashes. No duplicate
  metadata in the IR: the effective path carries the composition; the
  controller's own path stays in `Controller.path`.
- **Sync compatibility**: identity is computed from the effective path, so
  adding or changing a prefix/version updates requests through the normal
  matching tiers (an unchanged handler with a changed URL updates in
  place or moves like any path edit). Unversioned, prefix-less projects
  produce byte-identical output to v0.3.0 (the bootstrap analysis finds
  nothing to apply).

## Verification

- 8 unit tests for the bootstrap analysis (`nest::app`): prefix extraction
  from the bootstrap body, exclude rules (exact/method/wildcard), dynamic
  arguments, URI versioning with precedence and variants, VERSION_NEUTRAL,
  header strategies, provenance, discovery.
- 7 end-to-end tests (`tests/app_config.rs`) through the real binary:
  prefix URLs, excludes, `/{prefix}/v{n}/{path}` composition, precedence
  (controller '2' / method '3' / neutral), multi-version variant requests,
  non-URI strategies, configured bootstrap override.
- Full suite: 625 tests; fmt and clippy `-D warnings` clean.

## Known limits (accepted)

- Watch mode does not watch the bootstrap file: changing
  `setGlobalPrefix`/`enableVersioning` while `watch` runs needs a restart
  (tracked in `KNOWN_ISSUES.md`). Staged sync reads the bootstrap from the
  index snapshot when staged.
- Bootstrap analysis is one file; multi-app workspaces with different
  prefixes per app are not modeled.
- `bootstrap()` invoked through wrappers or decorators is not recognized.
