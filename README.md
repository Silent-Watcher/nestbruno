# NestBruno

NestBruno is a Rust command-line tool that keeps a [Bruno](https://www.usebruno.com/)
API collection synchronized with the HTTP routes of a NestJS project. It
analyzes the TypeScript source statically — your application is never
compiled, booted, or executed, no Node process is spawned, and no network
call is made — and reconciles the resulting route inventory against the
`.bru` files in a Bruno collection.

## How it works

The crate is a library with a thin CLI wrapper. Analysis flows through a
fixed layer boundary:

```
discovery -> oxc parser -> route IR -> Bruno model -> serializer -> writer
```

- **Discovery** walks the configured source directory and selects
  controller-candidate files with include/exclude globs (`node_modules`,
  `.git`, and configured excludes are pruned).
- **Extraction** parses each candidate with the [oxc](https://github.com/oxc-project/oxc)
  parser and reads decorators syntactically: `@Controller`,
  `@Get`/`@Post`/`@Put`/`@Patch`/`@Delete`/`@Head`/`@Options`,
  and the parameter decorators `@Body`, `@Query`, `@Param`, `@Headers`.
  Aliased (`Controller as C`) and namespaced (`common.Controller`)
  decorators are matched by name.
- **DTO resolution** turns `@Body()` type annotations into JSON body
  skeletons, same-file or across local imports.
- **Reconciliation** plans a pure diff between the desired requests and the
  existing collection, then applies it with per-write re-verification.
  Generated requests carry an ownership stamp in their `docs` block; files
  without a matching stamp, or whose content hash no longer matches, are
  treated as user-owned and are reported as conflicts instead of modified.
- **Writing** is the only component that touches the collection. Writes are
  atomic (temporary file plus rename).

## Features

- Static analysis only: TypeScript is parsed, never executed.
- Route extraction with path composition (`@Controller('x')` + `@Get('y')`
  -> `GET /x/y`), parameter metadata, and per-route source provenance.
- Cross-file DTO resolution: follows local imports to exported classes,
  interfaces, type aliases, and Zod schemas, and generates a JSON body
  skeleton (`null` for every value, at most 64 fields per DTO).
- Zod support: `z.object(...)`, provenance-checked `z.infer<typeof Schema>`
  type aliases (same file or cross-file, any declaration order), and
  `class X extends createZodDto(Schema)` (named, aliased, or
  namespace-imported from a real `nestjs-zod` import), plus statically
  provable multi-step transformation chains (`extend`, `pick`, `omit`,
  `partial`, `required`, `strict`, `passthrough`, `strip`); anything
  unprovable stays unknown.
- `package.json` `imports` subpaths: local `#src/...` specifiers resolve
  through exact and single-wildcard mappings; external targets are never
  resolved.
- Safe synchronization: ownership stamps, content-hash verification,
  conflict reporting, atomic writes. Hand-edited requests are never
  overwritten or deleted.
- Deterministic output: no timestamps, random identifiers, or
  filesystem-order dependence; identical input produces byte-identical
  collection state, suitable for Git diffs.
- Watch mode with bounded debounce and incremental reconciliation.
- Git staged sync (`sync --staged`): reconstructs the desired state from
  the Git index for controller files you have staged.
- Diagnostics with severity semantics: an `Error` diagnostic means "this
  path's result is unknown, not empty"; destructive synchronization is
  gated on completeness for that reason.

## Requirements

- Rust 1.91 or newer (edition 2024; the MSRV is pinned by oxc 0.116).

## Installation

From crates.io:

```sh
cargo install nestbruno
```

From a source checkout:

```sh
cargo install --path .
```

## Quick start

Run inside the NestJS project root (the directory containing `src/`):

```sh
nestbruno init      # write nestbruno.toml with the defaults (never overwrites)
nestbruno sync      # create/refresh bruno/ from the current routes
```

Define a `baseUrl` variable in a Bruno environment (for example
`http://localhost:3000`); generated request URLs start with `{{baseUrl}}`
by default. Then edit code and run `nestbruno sync` again, or keep it
running with `nestbruno watch`.

## Command reference

| Command | Behavior |
| --- | --- |
| `nestbruno init` | Writes `nestbruno.toml` with documented defaults. Never overwrites an existing file. |
| `nestbruno check` | Validates the configuration and the source directory layout. |
| `nestbruno discover` | Lists the files that match the include/exclude rules and may contain controllers. |
| `nestbruno routes` | Prints the extracted routes with parameters and source locations. |
| `nestbruno generate [--force]` | Writes one `.bru` request per route. Existing files are never modified without `--force`. |
| `nestbruno sync [--dry-run] [--staged]` | Reconciles the collection safely: creates, updates in place, deletes stale generated requests, preserves conflicts and user edits. `--dry-run` prints the plan without writing. `--staged` builds the desired state from the Git index for staged controller files. |
| `nestbruno watch [--debounce MS]` | Watches the source tree and reconciles incrementally (default debounce 200 ms, bounded 10-60000). Full resynchronization on changes it cannot apply incrementally. |

The global flag `--project/-C DIR` runs against `DIR` instead of searching
upward from the working directory for `nestbruno.toml`.

## Configuration

`nestbruno init` writes the following defaults; every key is optional:

```toml
# NestBruno configuration.

# Directory containing the NestJS sources, relative to this file.
source = "src"

# Bruno collection directory, relative to this file.
bruno = "bruno"

# Prefix of every generated request URL. The default is a Bruno variable
# you define in a Bruno environment (e.g. baseUrl = http://localhost:3000).
base_url = "{{baseUrl}}"

# Glob patterns, matched against paths relative to `source`, selecting the
# files that may contain controllers.
include = ["**/*.controller.ts"]

# A path matching any exclude pattern is skipped even if it matches `include`.
exclude = [
    "**/dist/**",
    "**/build/**",
    "**/coverage/**",
    "**/.tmp/**",
    "**/.cache/**",
]
```

Import resolution additionally reads a `tsconfig.json` at the project root
when present (JSONC syntax supported): `baseUrl` and `paths` mappings with
exactly one wildcard per pattern are honored. `extends`, `rootDirs`,
`package.json` `imports`/`exports`, and `node_modules` are not resolved.

## Route and DTO extraction

Supported:

- Top-level controller classes with call-form decorators; route paths may
  be static strings. `@All` routes are extracted and reported but not
  generated (Bruno has no request type that matches every method).
- `@Body()` parameters typed with exported classes, interfaces, type
  aliases, or Zod schemas; named `@Body('field')` parameters are merged
  into the body skeleton.
- Cross-file resolution through relative imports (with or without the
  `.ts` extension, including NodeNext-style emitted extensions:
  `./x.dto.js` resolves to `x.dto.ts`, likewise `.mjs`/`.cjs` to
  `.mts`/`.cts`), directory `index.ts`, `tsconfig` `baseUrl`/`paths`, and
  named, type-only, aliased, and re-exported bindings (up to four re-export
  hops).
- Resolution is bounded: at most 512 dependency files, 4 MiB aggregate
  dependency source, and 1024 DTO declarations per run; cycles are detected
  and reported.

Conservative by design: anything the analyzer cannot prove statically —
unsupported Zod transformations, ambiguous bindings, missing imports,
circular references — yields `BodyShape::Unknown`, rendered as an empty
JSON object with an explanatory `docs` entry, never as a guessed shape.

## Output format

One `.bru` file per route, grouped into folders per controller. Generated
requests carry provenance and an ownership stamp in their `docs` block:

```bru
meta {
  name: POST /enrollments/verify
  type: http
  seq: 2
}

post {
  url: {{baseUrl}}/enrollments/verify
  body: json
  auth: inherit
}

body:json {
  {
    "phoneNumber": null,
    "otp": null
  }
}

docs {
  NestJS: EnrollmentsController.verify
  Source: src/modules/enrollments/enrollments.controller.ts
  @Body(): type VerifyRequestBody
  NestBruno: v1 id=fe7d34d55bd9ce3d key=a1135ffb4b9f0570 anchor=ee0b0ab664a0ca1c hash=c1aaf3aeec8ba74c
}
```

The `NestBruno` line is the ownership stamp: it identifies the generated
request across renames and routes. Sync only modifies files whose stamp
matches and whose content hash is unchanged; anything else is a conflict
that is reported, not silently rewritten.

## Benchmarks

Criterion component benchmarks ship in `benches/pipeline.rs`. Medians,
measured 2026-11-10 (release profile; Intel i7-1255U, Linux, rustc 1.99.0):

| Benchmark | Median | Workload |
| --- | ---: | --- |
| `extract_cross_file/1_controller_1_shared_dto` | 30 us | One controller (3 routes); the `@Post` body type resolved from a DTO class in a separate file through a relative import |
| `extract_cross_file/8_controllers_1_shared_dto` | 79 us | Eight controllers sharing one DTO through a single run-scoped resolver context |
| `extract/100_controllers` | 556 us | 100 controllers / 300 routes extracted from a single ~24 KiB source |
| `generate/100_controllers_300_routes` | 2.5 ms | Pure planner: route IR to 300 desired `.bru` requests |
| `discover/1000_files_334_controllers` | 11.3 ms | Walk of a 1,000-file tree with `node_modules` noise |
| `sync/no_op_750_requests` | 13.0 ms | 750-request collection, nothing changed |

These are per-stage measurements, not wall-clock timings of a full run;
absolute values vary with hardware and cache state, while the scaling
relationships (extraction is microsecond-scale per file; generation and
sync are linear in request count) are the durable claims. Reproduce with
`cargo bench`; Criterion writes full reports to `target/criterion/`. See
[website/guide/benchmarks.md](website/guide/benchmarks.md) for methodology.

## Limitations

- Name-based decorator matching only; top-level classes only.
- Static global prefix and URI versioning only; `RouterModule` path composition and dynamic bootstrap expressions are not modeled.
- `@All`, array-valued paths, and path-to-regexp-style parameter patterns
  are extracted but not generated.
- Staged sync resolves same-file DTOs only; watch mode does not yet
  invalidate controllers when an imported DTO file changes.
- Source files are capped at 256 KiB; DTO skeletons are field names only,
  never types, defaults, or validation rules.

The maintained list, including rationale, is [KNOWN_ISSUES.md](KNOWN_ISSUES.md).

## Documentation

- [docs/architecture.md](docs/architecture.md) — layer boundary and design
- [docs/project-status.md](docs/project-status.md) — implemented capabilities and current limitations
- [docs/roadmap.md](docs/roadmap.md) — planned phases
- [docs/security.md](docs/security.md) — the static-analysis security model
- [docs/contributing.md](docs/contributing.md) and [docs/development.md](docs/development.md) — development workflow and quality gates
- [docs/phases/](docs/phases/) — per-phase implementation records

## License

[MIT](LICENSE)
