# Configuration

NestBruno reads one file, `nestbruno.toml`, from the project root. Every key
is optional; unknown keys are rejected (typos are errors, not silent
defaults).

```toml
# nestbruno.toml
source = "src"          # where to look for controllers
bruno = "bruno"         # the Bruno collection directory
base_url = "{{baseUrl}}"# URL prefix of generated requests
include = ["**/*.controller.ts"]
exclude = [
  "**/dist/**",
  "**/build/**",
  "**/coverage/**",
  "**/.tmp/**",
  "**/.cache/**",
]
# bootstrap = "src/main.ts"  # optional: where setGlobalPrefix/enableVersioning live
```

## Keys

| Key | Type | Default | Validation |
| --- | --- | --- | --- |
| `source` | path | `"src"` | relative, no `..`, must be an existing directory |
| `bruno` | path | `"bruno"` | relative, no `..`, must not be the project root itself |
| `base_url` | string | `"<span v-pre>{{baseUrl}}</span>"` | no control characters; single line |
| `include` | string list | `["**/*.controller.ts"]` | must not be empty; valid globs |
| `exclude` | string list | dist / build / coverage / .tmp / .cache | valid globs |
| `bootstrap` | path | `src/main.ts`, then `main.ts` (conventional candidates) | relative, no `..`; a configured file that does not exist is a warning |

An invalid value (wrong type, `..`, absolute path, empty `include`, bad
glob) is a configuration error: exit code 3.

## Include / exclude semantics

- Patterns match the path **relative to `source`**.
- `*` does not cross `/`; `**` does; matching is case-sensitive.
- A file is a candidate when it is **included and not excluded** — exclusion
  wins.
- A directory matched by an exclude pattern (including partial ones like
  `**/dist/**`) is not entered at all.
- `.git` and `node_modules` are always skipped and cannot be re-enabled.
- Only `.ts` files are candidates; `.d.ts`, `.tsx` and JavaScript are not.

## Bootstrap discovery

The bootstrap file is where `app.setGlobalPrefix(...)` and
`app.enableVersioning(...)` are analyzed (see [Routes](/guide/routes)). By
default the first conventional candidate that exists is used — `src/main.ts`,
then `main.ts`. Set `bootstrap` to point at a different entry file; a
configured file that is missing or unreadable is reported as a warning and
no prefix or versioning is applied.

Because matching is relative to `source`, a project that happens to live
under a directory named `dist`, or a source subdirectory named `build`, is
not accidentally ignored — only *nested* `dist`/`build`/… directories inside
the source tree are excluded by default.

## The collection directory

`bruno/` is created on first write. NestBruno creates `bruno.json` only if
absent (with collection name `"API"` and version `"1"`) and never replaces
it — the collection metadata is yours. No `folder.bru` and no environment
file are generated: both are user-owned. Define `baseUrl` in a Bruno
environment yourself, or set `base_url` in `nestbruno.toml` to a concrete
URL such as `http://localhost:3000`.
