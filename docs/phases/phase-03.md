# Phase 3: NestJS AST and route extraction

## Objective

Statically extract controllers and routes from the files found by Phase 2 into
a stable route intermediate representation (IR) that knows nothing about Bruno.

```text
Discovery ─▶ read ─▶ oxc AST ─▶ NestJS semantic walk ─▶ Route IR
```

## Parser architecture

- **Parser:** [`oxc_parser`](https://crates.io/crates/oxc_parser) 0.116 with
  `oxc_ast`, `oxc_allocator`, `oxc_span` (MIT). Evaluation: it parses modern
  TypeScript including decorators, reports syntax errors with spans, allocates
  the AST in an arena (cheap to drop per file) and is widely used and actively
  maintained. A C-based parser (tree-sitter) would break "written entirely in
  Rust"; `swc` is a much larger dependency for the same need. 0.116 is the
  newest release that builds on the Rust 1.91 toolchain used here, which sets
  `rust-version = "1.91"`. The four crates are one release train.
- **One parser, no regex** for any semantic extraction. Line/column come from
  AST spans (offset to position via a line table built lazily per file).
- `SourceType::ts()` (`.ts`, no JSX).
- The pure function `nest::extract::extract_controllers(file, source)` does the
  work; `nest::analyze_files` does I/O, threading and aggregation. The IR in
  `route.rs` has no dependency on either.

## NestJS semantic analysis

- **Controller:** top-level class (plain, `export`, `export default`) with a
  call-form decorator named `Controller`. The class name is irrelevant.
  Argument forms: none, a string, a substitution-free template literal, or an
  options object (`{ path: 'x', version: '1' }`).
- **Route:** non-static, non-abstract `method` of a controller with a call-form
  decorator named `Get`, `Post`, `Put`, `Patch`, `Delete`, `Head`, `Options`
  or `All`.
- **Parameters:** `Param`, `Query`, `Body`, `Headers` on formal parameters.
  `index` is the position among *all* formal parameters. `@Param()` gives
  `Unnamed`; `@Param('id')` gives `Named("id")`; `@Body(new ValidationPipe())`
  gives `Unnamed` (a `new` expression is a pipe, never a key); any other
  non-string first argument gives `Unresolved` with a warning.
- Anything else (guards, `@ApiTags`, `@Roles`, `@Req`, ...) is ignored.

## Route IR

```rust
pub struct Controller { name: String, path: RoutePath, source: SourceLocation, routes: Vec<Route> }
pub struct Route {
    method_name: String, http_method: HttpMethod, path: RoutePath,   // full path
    parameters: Vec<RouteParameter>, source: SourceLocation,
}
pub struct RouteParameter { kind: ParameterKind, name: ParameterName, index: usize }
pub enum HttpMethod { Get, Post, Put, Patch, Delete, Head, Options, All }
pub enum RoutePath { Static(String), Unresolved }
pub enum ParameterName { Unnamed, Named(String), Unresolved }
pub struct SourceLocation { file: Arc<Path>, line: u32, column: u32 }   // 1-based
```

Differences from the prompt's sketch, and why:

- `Route` has no `controller` field: a route lives inside its `Controller`,
  so the field would duplicate data. (A flat view is a trivial iterator.)
- `RoutePath` / `ParameterName` carry an explicit `Unresolved` state so an
  unknown value can never be mistaken for an empty one (`@Param()`).
- `file` is an `Arc<Path>` shared by all locations of a file (no per-route
  path allocation).

**Locations** point at the `@Controller` / route decorator (`file:line:col`,
1-based; columns count Unicode scalar values; `\n`, `\r\n`, `\r` end lines).
`file` is relative to the project root.

**Path normalization** (`compose_path`): split on `/`, drop empty segments,
join with `/`, prefix `/`; root is `/`. Nothing else is touched (`:id`, `:id?`,
`*`, case, `..` are preserved, so `/users/:id` stays Nest-style).
`/users/` + `/:id/` gives `/users/:id`. It is idempotent and splittable
(property tested).

**Ordering:** files in discovery order; within a file source order of classes,
methods and parameters (decorator order within one parameter).

## Unsupported behavior

- Dynamic expressions (`@Controller(prefix)`, `@Get(build())`, concatenation,
  `as const`, parentheses, substitutions in templates, spreads, computed
  option keys): reported as unresolved with a warning, never evaluated.
- Array paths (`@Get(['a','b'])`): unresolved (one route per path is future work).
- Custom decorators (`@MyGet('/x')`), aliased imports (`Controller as C`),
  namespace imports (`common.Get`): not recognized. Decorators are matched by
  name and the import origin is not checked.
- Several route decorators on one method: the first is extracted, with a warning.
- Computed method names: skipped with a warning (string-literal keys work).
- Anonymous controller classes: skipped with a warning.
- Nested classes, controller inheritance, global prefix, versioning,
  `RouterModule`, `@Sse`, DTO schemas, Swagger: not modelled.

## Error handling

| Situation | Result |
| --- | --- |
| Syntax error | `error` diagnostic with `file:line:col`; **no routes** from that file; other files continue; exit 4 |
| Unreadable, non-UTF-8, non-regular or > 256 KiB file | `error` diagnostic; exit 4 |
| Unsupported / dynamic construct | `warning`; exit unaffected |
| Candidate without `@Controller` | `note` (verbose only) |
| Cannot start the worker thread | fatal, exit 1 |

An `error` diagnostic means the file's routes are **unknown**, not absent.
Phase 5 must not delete Bruno requests belonging to such a file.

## Security

Source is untrusted. It is parsed, never executed: no evaluation, no Node.js,
no subprocess, no network, no `unsafe` (forbidden crate-wide). Hazards handled:

- **Stack overflow.** The parser has no recursion limit, and a stack overflow
  aborts the process. Measured (bisection over 25 nesting constructs, 8 MiB
  stack): the worst case is ~1394 B of stack per input byte (debug) / ~559 B
  (release), on nested parentheses. Each nesting level needs at least one input
  byte, so capping input at 256 KiB and giving the analysis thread a stack of
  2048 B per cap byte (512 MiB of lazily committed virtual memory) bounds the
  need with a ~1.5x margin over the debug worst case. A test feeds worst-case
  files of ten nesting kinds at exactly the cap through `analyze_files` in a
  debug build; shrinking the stack to 8 MiB makes it abort (verified).
  This bound is empirical for the pinned version; see security.md.
- **Panics** in the parser are caught per file.
- **Files:** bounded read, UTF-8 check, `symlink_metadata` regular-file check.

## Performance

Each file is read once and parsed once; the AST lives only while its IR is
built; locations are computed from spans through a lazily built line table
(files without controllers never build it); no JSON/AST conversion; no regex;
no caching.

Single ad-hoc measurement (release build, Linux sandbox, warm cache): 5,000
controller files (2.8 MB, 65,000 output lines) took about 130 ms end to end
including discovery (about 30 ms of it), peak RSS about 17 MB. This is an
observation, not a benchmark suite (Phase 10). Parsing is single-threaded.

## Testing

- `route.rs`: documented path examples; preservation of Nest syntax;
  method/kind mappings; property tests: normalized, idempotent, splittable,
  segment-preserving.
- `nest/extract` (52 tests): every controller and method decorator; every
  parameter form and index rule; formatting equivalence (multiline, trailing
  comma, comments, quotes, template); multiple controllers per file; export
  forms; unsupported/custom/aliased decorators; every dynamic form; multiple
  route decorators; computed names; surrounding TypeScript syntax; CRLF and
  Unicode locations; malformed input; BOM; determinism. Property tests:
  arbitrary text and mutated valid source never panic and are deterministic;
  generated paths round-trip through `compose_path`.
- `tests/routes.rs` (14): full project through the binary with exact expected
  output; ordering; nested dirs; no-controller candidates; custom `include`;
  malformed file among valid ones (exit 4, valid routes kept); unresolved
  values; invalid UTF-8; oversize; worst-case nesting at the cap; 100,000-deep
  nesting via the CLI; files swapped for symlinks/directories/missing.
- Regression: the stack-overflow finding above is permanently covered.

## Limitations

See *Unsupported behavior*. Additionally: output of `routes` is a diagnostic
text format, not a stable interface; extracted paths do not include global
prefixes or versioning, so they can differ from the live application's.

## Acceptance criteria

All items of the Phase 3 prompt's section 49 are met with the limitations
listed; fmt, clippy (`-D warnings`) and the 161 tests pass.

## Commits

See `git log`: `feat: add NestJS AST route extraction`,
`test: add route extraction integration tests`, `docs: document phase 3`.

## What Phase 4 builds on

Phase 4 consumes `Analysis.controllers` (`Vec<Controller>`): map each
controller to a Bruno folder, each `Route` to a request (method, `path`,
parameters). It must handle `RoutePath::Unresolved` and `ParameterName::
Unresolved` explicitly, treat files with `error` diagnostics as unknown, and
sanitize every name taken from source before using it in a file path
(`Controller.name`, `method_name`, and path text are untrusted).
