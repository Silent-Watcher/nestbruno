# Phase 4: Bruno collection generation

## Objective

Turn the route IR into valid, deterministic Bruno `.bru` files, written safely
into the configured collection directory. Generation only: no reconciliation,
merging, deletion, watch mode, Git integration or caching.

```text
Discovery -> NestJS analysis -> Route IR -> Bruno model -> serializer -> writer
```

## Bruno format

**Sources read (2026-10-08):** the Bruno repository, `packages/bruno-lang/v2`
(`jsonToBru.js` serializer, `bruToJson.js` grammar and semantic actions,
`utils.js`, `collectionBruToJson.js`), the library's canonical fixture
`tests/fixtures/request.bru`, Bruno's own `bruno-tests/collection/bruno.json`,
and `bruno-electron` request preparation (for `auth: inherit`). The repository's
`bruno-lang/package.json` reports version 0.12.0.

**Validation against the real parser:** every generated file was parsed with
the published npm package `@usebruno/lang` 0.40.0 (`bruToJson`), the parsed
fields were checked (method, URL, enabled/disabled flags, params, body), and
Bruno's own serializer (`jsonToBru`) was run on the parsed result: it
reproduced our bytes **exactly** for all files checked (the 9-file fixture, a
5-file hostile-input run, and a 500-file run). This script is a one-off
verification aid and is deliberately not part of the Rust repository or its
(offline) test suite; the golden fixtures plus the layout tests lock the result
in.

**Not verified:** opening the collection in the Bruno desktop app (no GUI
here). Behavior of older Bruno versions. `auth: inherit` is read from Bruno's
source and is not recognized by very old versions.

Facts that shape the implementation:

| Fact | Consequence |
| --- | --- |
| A value runs to end of line; a block ends at a newline followed by `}`; blocks such as `script:pre-request` contain executable JavaScript | A line break in any source-derived string could inject code. All such text is validated; see Security |
| Bruno interpolates `{{...}}` in URLs, params and bodies, including `{{process.env.X}}` | `{{` from source is never emitted |
| A leading `~` on a key means "disabled"; quoted keys only support `\"` | Names starting with `~`/`@` or containing `\` are not representable and are left out with a warning |
| Bruno re-saves files from its own model and drops unknown `meta` keys | No custom metadata in `meta`; traceability lives in the retained `docs` block |
| There is no "any method" request type | `@All` is not generated (warning) |
| Standard methods are lowercase blocks: `get`, `post`, `put`, `patch`, `delete`, `head`, `options` | Direct mapping |

## Route to Bruno mapping

```text
<bruno>/
  bruno.json            created only if absent; never overwritten, even with --force
  <Controller>/         one folder per controller class
    <handler>.bru       one file per route
```

- No `folder.bru` and no environment file are generated: they are files users
  customize, and ownership rules arrive in Phase 5. Define the `baseUrl`
  variable in a Bruno environment yourself (or set `base_url` in
  `nestbruno.toml`).
- A generated request:

```text
meta { name: GET /users/:id, type: http, seq: 2 }
get  { url: {{baseUrl}}/users/:id, body: none, auth: inherit }
params:query { ~include:  }          # disabled: visible and editable, not sent
params:path  { id:  }
headers      { ~authorization:  }    # disabled
body:json    { {} }                  # or { "field": null, ... }
docs         { NestJS: UsersController.findOne / Source: src/... / notes }
```

- **Method:** direct; `@All` skipped with a warning.
- **URL:** `base_url` (default `{{baseUrl}}`, trailing `/` trimmed) + the route
  path. Path parameters (`:name` segments made of letters, digits, `_`) are
  kept in the URL and listed in `params:path`; they come from the URL, as in
  Bruno, not from `@Param`. An optional `:id?` becomes `:id` (a `?` would start
  a query string) and is noted in `docs`. Other segments (`*`, `:name.json`)
  stay verbatim without a params entry. Paths containing `?`, `#`, whitespace
  or `{{` are not generated.
- **Query and headers:** `@Query('x')` / `@Headers('x')` become *disabled*
  entries with empty values, so no invented value is ever sent. Unnamed
  `@Query()` / `@Headers()` / unresolved names add a line to `docs`; no fields
  are invented. Duplicates are removed (headers case-insensitively).
- **Body:** any `@Body` parameter gives `body: json`; named `@Body('f')` give
  one `"f": null` per field (`null` = unknown type), otherwise `{}`. No DTO is
  inspected. No `@Body` gives `body: none`.
- **Auth:** `auth: inherit` (the collection's auth applies); no authentication
  is inferred or created.
- **`seq`:** 1-based position among the controller's generated requests, in
  source order. Treat it as user-owned after creation (users reorder in Bruno).
- **Request name:** `METHOD /path`.

## Naming

Implemented in `bruno/naming.rs` (rules also in its module docs).

- **File:** handler name in kebab-case, ASCII only (`findOne` to `find-one`,
  `getUserByID` to `get-user-by-id`); empty gives `route`. English inference is
  not used. Route path text is never part of a file name.
- **Folder:** class name without a trailing `Controller`, reduced to
  `[A-Za-z0-9_-]`; empty gives `Controller`.
- Reserved Windows names (`CON`, `NUL`, `COM1`...) get a trailing `_`; stems
  are at most 60 characters; names compared case-insensitively.
- **Collisions:** items that share a name (case-insensitively) **all** get
  `-<8 hex digits of their fingerprint>`, so the result does not depend on
  input order; a residual duplicate gets `-2`, `-3`. No randomness, time,
  addresses or filesystem order. Trade-off: introducing a colliding route
  renames the previously unsuffixed file; accepted for order independence and
  documented for Phase 5.

## Route identity

`RouteIdentity` = controller (class name + project-relative file) + HTTP method
+ normalized path + handler name, with a stable 64-bit FNV-1a fingerprint
(pinned by test and cross-checked against an independent implementation).
Nothing machine specific. Phase 4 did not store it in the file; **Phase 5 adds
a stamp line to the `docs` block** (`NestBruno: v1 id=... key=... anchor=...
hash=...`, see [phase-05.md](phase-05.md)) because Bruno drops unknown `meta`
keys but keeps `docs`. Documented limits: renaming a handler or a
path parameter, or moving a controller file, changes the fingerprint. Phase 5
must match on the identity *fields* (method + path is the strongest signal),
not on the fingerprint alone. The `docs` lines other than the Phase 5 stamp are
a user-editable hint, not authority.

## Existing files

Exactly what happens, per file (never parsed or merged):

| Situation | Without `--force` | With `--force` |
| --- | --- | --- |
| Does not exist | created | created |
| Exists, identical bytes | unchanged (not rewritten) | unchanged |
| Exists, different bytes | **left untouched**, warning | replaced atomically |
| Is a symlink / directory / special file | error, never touched | error, never touched |
| `bruno.json` exists | left alone | left alone |

Files of routes that no longer exist are **not** removed. Files in the
collection that were not generated (`folder.bru`, environments) are never
touched.

## Security

- **Untrusted text never becomes syntax.** Control characters, Unicode line
  separators and `{{` in paths and names are rejected in the model; the
  serializer independently refuses control characters, `'''` openers and
  unrepresentable keys, and returns no text at all on failure. Body lines are
  indented so a `}` cannot end a block; JSON keys are escaped.
- **Paths.** Generated components come from a fixed ASCII alphabet, and the
  writer re-validates every component (no `/`, `\`, `:`, `..`, control
  characters, trailing dot/space, > 255 bytes) before use. The collection
  directory is validated by `config` and walked from the project root.
- **Symlinks.** A symlink anywhere on the collection path aborts; a symlinked
  folder fails only its files; an existing symlink target is never written
  through (also not when dangling).
- **Atomic writes.** Temp file in the target directory, written fully and
  `fsync`ed, then no-clobber link (new) or rename (replace). A failed write
  leaves the old file; a failed serialization produces no file.
- **Failure scope.** Atomic per file. A file/folder failure is reported and
  the rest continue (exit 4); failure to establish the collection directory
  aborts before writing anything.
- Mutation-checked: disabling the control-character guard or the `{{` guard
  makes the injection integration test fail.

Limits: no protection against another process swapping a directory for a
symlink between check and write (check-then-write); the parent directory is not
`fsync`ed; leftover `.nestbruno-*.tmp` files are possible after a crash.

## Performance

Generation works only on the route IR (no parsing, no source reads, no
existing-file parsing). Names are resolved with hash maps (linear); directory
checks run once per folder, not per file; identical existing files are
detected by length first and then bytes, and are not rewritten.

Measured (release build, Linux sandbox, single runs, warm cache): 5,000 routes
in 50 controllers: parse+extract 0.03 s; first generation (5,000 files) about
3.1 s; repeat run (all unchanged) 0.06 s; peak RSS about 13 MB. The first run
is dominated by file creation: `fsync` per file accounts for about 1.7 s of the
3.1 s (measured by removing it: 1.4 s). It is kept deliberately for crash
safety, and costs only on first generation. This is an observation, not a
benchmark suite (Phase 10).

## Testing

- Unit: naming (kebab cases, reserved names, truncation, collision
  order-independence, properties), identity (FNV vectors, pinned fingerprint,
  field sensitivity), model (URLs, params, optional segments, body, `@All`,
  hostile paths and names), serializer (golden layout for all methods,
  hazardous values, determinism), writer (create/skip/force/unchanged,
  symlinks, directories, unsafe components, modes, no temp leftovers, failure
  leaves old file), planner (folders, collisions, hostile names).
- Golden: `tests/fixtures/nest-project` generates exactly
  `tests/fixtures/expected-bruno` (byte for byte, exact file set).
  `.gitattributes` protects the trailing spaces and line endings.
- Integration (`tests/generate.rs`, real binary): determinism across
  directories and creation order; idempotent second run; existing-file policy;
  `--force` scope; stale files kept; collisions; same-named controllers;
  custom `bruno`/`base_url`; empty project; malformed file; hostile source
  (newline/CRLF injection, `{{process.env}}`, traversal-looking and reserved
  names, NUL, U+2028); symlinked collection/folder/file; 500 routes.
- Verification outside the suite: Bruno's real parser and serializer (above).

## Limitations

- No synchronization: stale requests stay; edits are never merged.
- `@All`, unresolved paths, array paths, paths with `?`/`#`/whitespace/`{{`
  are not generated. Global prefix and versioning are not modelled.
- Request bodies are skeletons only; no DTO analysis.
- No environment or `folder.bru`; `baseUrl` must be defined by the user.
- Names depend on the handler name; non-ASCII handler names become `route`.
- Windows behavior is untested.

## Acceptance criteria

All items of the Phase 4 prompt's section 50 are met within the limitations
above; fmt, clippy (`-D warnings`) and the full test suite pass.

## Commits

See `git log`: `feat: generate Bruno requests from NestJS routes`,
`test: add Bruno generation tests and golden fixtures`,
`docs: document phase 4`.

## What Phase 5 builds on

`bruno::generate` already yields the *desired state* as data
(`Vec<GeneratedRequest>` with identity and full content) separately from
writing, which is the input reconciliation needs. Phase 5 must define
tool-owned vs user-owned fields (candidates: tool-owned `url`, method, path
params, `docs`; user-owned `seq`, `auth`, headers values, scripts, tests, vars,
body content once edited), parse existing `.bru` files, match them to
`RouteIdentity` by fields, and treat files with `error` diagnostics from
analysis as unknown (never delete on unknown). It replaces the blunt
`--force`/skip policy and the `docs`-as-hint convention.
