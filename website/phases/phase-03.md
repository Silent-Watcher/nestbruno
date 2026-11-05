# Phase 3 — Route Extraction

**Ships:** `nestbruno routes` — static analysis of TypeScript into a stable
route model, and the internal Route IR everything else consumes.

```text
candidate files → read → oxc AST → NestJS semantic walk → Route IR
```

## Parser choice

[`oxc`](https://oxc.rs/) 0.116: a mature, pure-Rust TypeScript parser with
decorator support, error spans and arena allocation (the AST is dropped as
soon as a file's IR is built). One parser, no regex for any semantic
extraction; line/column always come from AST spans.

## What is recognized

- `@Controller` on top-level classes — no argument, a string, a
  substitution-free template, or `{ path: ... }`.
- `@Get @Post @Put @Patch @Delete @Head @Options @All` on methods.
- `@Param @Query @Body @Headers` on parameters, with names and the
  parameter index among *all* formal parameters.
- Paths are composed and normalized (`/users/` + `/:id/` → `/users/:id`;
  `:id`, `:id?`, `*` and case preserved — Nest semantics stay Nest-style).

Everything else (guards, Swagger, custom decorators) is ignored. Dynamic
arguments (`@Controller(prefix)`, `@Get(build())`) become **unresolved**
with a warning — never evaluated, never guessed.

## Safety engineering

Source is untrusted input. Highlights:

- **Stack-overflow defense:** worst-case parser stack use was measured
  (~1.4 KB per input byte on deeply nested input), so input is capped at
  256 KiB and the analysis thread gets a proportionally sized stack —
  verified by tests that feed worst-case files at the cap.
- Parser panics are caught per file; a syntax error means *no routes* from
  that file, one diagnostic with `file:line:col`, and exit 4 — other files
  continue.
- Bounded reads, UTF-8 checks, regular-file checks. No execution, no
  subprocesses, no network, no `unsafe`.

## Property tests

Path normalization is idempotent and splittable; arbitrary and mutated
source never panics and parses deterministically; identical source always
produces identical IR.

::: tip Full record
IR shape, unsupported constructs, the measured stack analysis and the full
test inventory: `docs/phases/phase-03.md` in the repository.
:::
