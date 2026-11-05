# Phase 4 — Bruno Generation

**Ships:** `nestbruno generate [--force]` — deterministic, valid `.bru`
files written safely into the collection.

## Format fidelity

The Bruno format was verified against Bruno's own parser/serializer
(`@usebruno/lang`): generated files round-trip byte-for-byte through
Bruno's `bruToJson` → `jsonToBru`. Facts that shape the output:

- <span v-pre>`{{...}}`</span> in URLs, params and bodies is interpolated by Bruno, including
  <span v-pre>`{{process.env.X}}`</span> — so <span v-pre>`{{`</span> from source is never emitted.
- A leading `~` marks a disabled entry — `@Query('page')` becomes
  `~page:`: visible and editable, never sent with an invented value.
- There is no "any method" request — `@All` routes are skipped with a
  warning and their requests preserved in later phases.
- Bruno drops unknown `meta` keys on re-save — so the ownership stamp
  (Phase 5) lives in `docs`.

## Mapping

One folder per controller class, one request per route:

```txt
meta { name: GET /users/:id, type: http, seq: 2 }
get  { url: {{baseUrl}}/users/:id, body: none, auth: inherit }
params:path  { id:  }
docs         { NestJS: UsersController.findOne / Source: src/... }
```

Path parameters come from the URL (as in Bruno), `@Body` gives a JSON
skeleton (`{}`, or one `null`-typed field per named `@Body('f')`), `seq` is
the source-order position, and `auth: inherit` defers to the collection —
no authentication is ever invented.

## Naming

Handler names become kebab-case file names (`findOne` → `find-one`); class
names become folders (minus `Controller`). Windows-reserved names are
escaped; name collisions are resolved deterministically — all colliding
items get `-<8 hex fingerprint>` suffixes, so the result never depends on
input order.

## Existing files are sacred

Existing identical files are not rewritten; differing files are left alone
without `--force`; symlinks and directories are never touched; `bruno.json`
is created once and never replaced; nothing is ever deleted. Writes are
atomic (temp file + fsync + rename): a failed write leaves the old file, a
failed serialization produces no file.

::: tip Full record
Format-verification methodology, security table, golden fixtures, measured
numbers: `docs/phases/phase-04.md` in the repository.
:::
