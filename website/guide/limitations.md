# Limitations & FAQ

## Current limitations

NestBruno reflects what is statically visible in decorators. Not modelled:

- **Name-based decorator matching** — aliased imports (`Controller as C`),
  namespace imports (`common.Get`) and custom decorators (`@MyGet`) are not
  recognized.
- **Top-level classes only** — controller inheritance is not followed.
- **No global prefix / versioning / `RouterModule`** — extracted paths can
  differ from the running application's routes.
- **Sources over 256 KiB are skipped** (reported as an error).
- **No field-level merge** — any user edit locks the whole request; a route
  change on an edited request is a conflict, not a merge. `seq` is
  user-owned after creation.
- **Nothing is deleted while any source file failed to analyze.**
- **`sync` reads the whole collection each run**; `watch` keeps an in-memory
  index but does not watch the collection (your Bruno edits are caught by
  write-time verification).
- **Not generated:** `@All` routes, unresolved/array paths, paths with `?`,
  `#`, whitespace or <span v-pre>`{{`</span>. Body examples are skeletons,
  not full validation schemas. DTO resolution has boundaries — see
  [DTO Resolution](/guide/dto-resolution) for the full supported subset
  (same-file DTOs must not use `export`; Zod chains are limited to one
  statically provable transformation; `createZodDto` classes must live in
  an imported module).
- **Watch mode does not invalidate on dependency-only changes** — editing an
  imported DTO file does not re-analyze controllers that import it until the
  controller itself changes. Run a plain `sync` to pick up dependency edits.
- **`sync --staged` resolves same-file DTOs only** — cross-file imports are
  not resolved from the Git index snapshot (dependency content would mix
  index and working-tree state).
- **No environment or `folder.bru`** — `baseUrl` must be defined by you.
- **Output verified with Bruno's parser library**, not by opening the
  desktop app; Windows/macOS are exercised in CI but were developed on
  Linux; watch end-to-end tests are Unix-only.

## FAQ

**Does NestBruno run my application?** No. Static analysis only — your
TypeScript is never executed (see [Safety](/guide/safety)).

**Will it overwrite the requests I edited?** No. An edited request is
detected by its content hash and becomes a conflict. Delete the file (or its
stamp line) to adopt the regenerated version.

**What happens to requests of removed routes?** Unmodified generated
requests are deleted; modified ones become conflicts; anything unknown is
preserved.

**Can I hand-write requests in the same collection?** Yes. Files without a
NestBruno stamp are preserved untouched, forever.

**Why is `@Query` generated as a *disabled* parameter?** So the parameter is
visible and editable in Bruno but no invented value is ever sent. Fill it in
and enable it.

**Why `{{baseUrl}}` and no environment file?** Environments are user-owned;
NestBruno creates none. Define `baseUrl` in Bruno, or set `base_url` in
`nestbruno.toml`.

**Does it watch `node_modules`?** No — never watched, never scanned.

**Is the output deterministic?** Yes: identical source and configuration
produce byte-identical files, so the collection is Git-diff friendly.

**What does exit code 4 mean?** "Finished, but incomplete": at least one
file could not be read/parsed. Fix the files named by `error:` diagnostics —
until then, no destructive operation runs.

**Where are the development details?** In the repository:
`docs/architecture.md`, `docs/phases/`, and the [Phases overview](/phases/).
