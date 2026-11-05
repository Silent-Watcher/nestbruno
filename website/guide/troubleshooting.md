# Troubleshooting

## Reading diagnostics

Results go to stdout; diagnostics go to stderr with a severity:

```txt
error:   src/users/users.controller.ts:17:3: unexpected token
warning: src/users/users.controller.ts:12:3: decorator argument is not a static string; route unresolved
note:    ... (shown only with --verbose)
```

An `error:` on a file means that file's routes are **unknown** (not "no
routes"): destructive actions are suppressed while any file is in this
state.

## Exit codes

| Code | Cause | Fix |
| --- | --- | --- |
| 2 | no / unknown arguments | see `nestbruno --help` |
| 3 | invalid config, missing source dir, source outside project | the message names the setting or path |
| 4 | some files could not be processed (see `error:` lines) | fix the named files; or accept a partial result |
| 1 | other runtime failure | the message includes the path |

## Common situations

**`error: configuration error ... unknown field`** — a typo in
`nestbruno.toml`; unknown keys are rejected. Run `nestbruno init` to see the
documented keys.

**`warning: ... @All ...`** — Bruno has no "any method" request; routes with
`@All` are not generated. Their existing requests are preserved, never
deleted.

**`CONFLICT ... generated file was modified`** — you edited a request whose
route also changed. Keep yours, or delete the file (or just its
`NestBruno:` stamp line) and run `sync` again. See
[Synchronization](/guide/synchronization#resolving-a-conflict).

**`critical conflicts found: nothing was written` / duplicate identity** —
usually Bruno's "clone request", which copies the stamp into the copy.
Delete the `NestBruno:` line in the clone (or the clone itself) and rerun.

**`watch` fails with a watch-limit error** — the OS limit on watched
directories was hit (`fs.inotify.max_user_watches` on Linux). Raise the
limit or narrow `source`.

**Requests not created for a route** — check the warning: the route may use
`@All`, a dynamic path (`@Get(buildUrl())`), or a path containing `?`, `#`,
whitespace or <span v-pre>`{{`</span>. Also check `include` — the file must match
`**/*.controller.ts` (or your configured patterns) to be seen at all.

**Extracted path differs from the running app** — global prefixes
(`app.setGlobalPrefix`), version URIs and `RouterModule` paths are not
modelled; the IR reflects decorators only.

**`routes` output changed format** — it is a diagnostic text format, not a
stable interface; don't parse it in scripts.

## Still stuck?

Run with `-v` for note-level detail, and
[`check`](/guide/commands#nestbruno-check) to validate the layout. The
repository's `docs/` (especially `docs/project-status.md`) lists all known
limitations.
