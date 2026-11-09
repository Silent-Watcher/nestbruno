# Introduction

**NestBruno** is a command-line tool that keeps a [Bruno](https://www.usebruno.com/) API
collection synchronized with the HTTP routes of a NestJS project.

It reads your TypeScript source with a real parser, finds your controllers and
routes, and creates, updates or deletes Bruno requests so that the collection
always mirrors the API you are actually building — while never destroying
anything you wrote by hand.

```text
NestJS source ─▶ discovery ─▶ AST analysis ─▶ route IR ─▶ Bruno requests
                                     ▲
                        only changed files on re-run
```

## Why static analysis

NestBruno never runs your application. Your TypeScript is parsed (with
[oxc](https://oxc.rs/)) and inspected, never executed: no Node.js, no
decorator evaluation, no network. That makes it safe to point at any
repository and fast enough for watch mode.

## Core concepts

- **Discovery** walks your configured `source` directory and lists candidate
  controller files from paths alone (`**/*.controller.ts` by default). It
  never reads file contents.
- **Route IR** is the internal model of what was found: controllers, HTTP
  methods, full paths (`/users/:id` stays Nest-style), parameters and source
  locations. Bruno never leaks into this layer.
- **Generation** turns the IR into deterministic `.bru` files: one folder per
  controller class, one request per route.
- **Ownership** is explicit. Every generated request carries a stamp in its
  `docs` block. A file that you edit is *yours*: NestBruno will report a
  conflict instead of overwriting it. Unknown files (hand-written requests,
  environments, `folder.bru`) are preserved.
- **Reconciliation** plans the whole change before touching the disk, then
  applies it with atomic, re-verified writes.

## Where to go next

- [Getting Started](/guide/getting-started) — install and first sync.
- [Commands](/guide/commands) — every command and exit code.
- [Configuration](/guide/configuration) — `nestbruno.toml` reference.
- [DTO Resolution](/guide/dto-resolution) — same-file, cross-file and Zod body skeletons.
- [Synchronization](/guide/synchronization) — what sync creates, updates, deletes and refuses to touch.
- [Watch Mode](/guide/watch-mode) — live synchronization while you code.
- [Safety & Ownership](/guide/safety) — the guarantees, in detail.
- [Benchmarks](/guide/benchmarks) — measured performance and methodology.
