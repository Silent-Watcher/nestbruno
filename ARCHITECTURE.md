# ARCHITECTURE.md — actual architecture of NestBruno (agent context)

This is the agent-facing architecture summary. The full narrative lives in
[docs/architecture.md](docs/architecture.md) (canonical) and the phase
documents under [docs/phases/](docs/phases/). The shipped architecture is
complete through Phase 9 plus Phase 11 (cross-file DTO resolution, shipped
in v0.2.0). See the Phase 11 section below and the canonical handoff before
modifying the resolver.

## What the tool is

`nestbruno` reads NestJS TypeScript source statically and keeps a Bruno API
collection (`.bru` files) synchronized with the HTTP routes it finds. Seven
commands: `init`, `check`, `discover`, `routes`, `generate [--force]`,
`sync [--dry-run]`, `watch [--debounce MS]`.

## Pipeline (actual)

```text
nestbruno.toml (+ Project::find_root)
        │
        ▼
discovery.rs      path-only candidate scan (never reads file contents)
        │  ControllerFile { path }  · sorted, deterministic
        ▼
nest/extract.rs   oxc 0.116 TypeScript AST → route IR  (the only parser)
   (via nest/mod.rs: bounded read, big-stack worker, panic guard per file)
        │  Controller { name, path, source, routes: Vec<Route> }
        ▼
bruno/mod.rs      generate(): pure planner, IR → Vec<GeneratedRequest>
   (model.rs mapping · naming.rs collision-safe names · identity.rs
    fingerprints · stamp.rs ownership stamp · serialize.rs .bru text)
        │
        ├──▶ bruno/writer.rs   generate: atomic writes, never deletes
        │
        └──▶ sync/*            sync: existing.rs classifies current .bru
                                  reconcile.rs plans (pure)
                                  apply.rs executes with re-verification
        │
        └──▶ watch/* + workspace.rs
             notify events → normalize → debounce → dirty set →
             Workspace::refresh (re-parse only dirty files) →
             same reconcile/apply over in-memory index
```

Module responsibilities table: [docs/architecture.md](docs/architecture.md).

## Key mechanisms an agent must not break

- **Route IR is the only boundary.** `src/route.rs` knows nothing about
  Bruno; `src/bruno/` knows nothing about TypeScript. Adding an output format
  (Phase 10 OpenAPI) means a new consumer of the IR, not a new parser.
- **Ownership stamp** (`bruno/stamp.rs`): every generated request ends its
  `docs` block with `NestBruno: v1 id=… key=… anchor=… hash=…`. `id` =
  full fingerprint; `key` = class+method+path (survives handler rename/file
  move); `anchor` = controller class+handler (survives path/method edit);
  `hash` = content hash of the file without the stamp line and `seq`. A file
  is "modified by user" iff its normalized content no longer hashes to
  `hash`. The stamp is a claim, always re-verified.
- **Reconciliation matching tiers** (`sync/reconcile.rs`): exact `id` →
  unique `key` → unique `anchor`, weaker tiers only when unique on both
  sides and not contradicted. Duplicate identities on either side block all
  writes (critical conflicts).
- **Diagnostic contract** (`diag.rs`): `Severity::Error` means "this path's
  result is *unknown*, not empty". Everything destructive is gated on
  `is_incomplete()` (no deletions while any source file failed; watch keeps
  last-good routes for broken files and sets `complete=false`).
- **Writer is the only writer** (`bruno/writer.rs`): containment checks,
  component validation, symlink refusal, temp file + rename + fsync,
  `bruno.json` created once and never replaced (hardcoded name `"API"`).
  Sync-only primitives re-verify the hash before update/delete.
- **Watch events are hints** (`watch/`, `workspace.rs`): a dirty path is
  re-statted and re-read; event order cannot misrepresent state. Lost
  events, overflow, or inconsistent state fall back to a full recompute
  from disk. Property test: any history of changes ≡ fresh sync of the
  final state.
- **No execution of user code, ever.** Static analysis only; see
  [docs/security.md](docs/security.md).

## Error model

Fatal `Error` enum (`error.rs`) → exit codes: 0 success, 1 failure, 2 clap
usage, 3 config/project, 4 "finished but incomplete" (via `Outcome::Incomplete`,
not an `Error`). Per-path problems become `Diagnostic`s on stderr; Notes are
verbose-only.

## Config

`nestbruno.toml`, all keys optional, unknown keys rejected: `source` (default
`"src"`), `bruno` (default `"bruno"`), `base_url` (default `"{{baseUrl}}"`),
`include` (default `["**/*.controller.ts"]`), `exclude` (default dist/build/
coverage/.tmp/.cache). Details: `src/config.rs`, [docs/phases/phase-01.md](docs/phases/phase-01.md)
(+ phase-04 for `base_url`).

## Build facts

- Rust edition 2024, `rust-version = "1.91"` (oxc 0.116 pins it).
- Runtime deps: clap, serde, toml, thiserror, tempfile, globset, oxc
  (allocator/ast/parser/span, one release train), notify, ctrlc.
  Dev: proptest. Rationale: `Cargo.toml` comments.
- Historical Phase 9 baseline: 495 tests. Phase 11 raised this to 535
  passing (verified 2026-11-09). Watch e2e tests are Unix-only.
- CI: fmt + clippy `-D warnings` + tests on linux/macos/windows
  (`.github/workflows/ci.yml`); exercised on Linux only so far.

## Known architectural limits

See [KNOWN_ISSUES.md](KNOWN_ISSUES.md) and the "Current limitations" section
of [docs/project-status.md](docs/project-status.md). Highlights: name-based
decorator matching, top-level classes only, no global prefix/versioning, 256
KiB source cap, no field-level merge (any user edit locks the whole request),
`sync` re-reads all `.bru` per run, linear watch reconciliation pass.


## Phase 11 cross-file DTO resolver — shipped (v0.2.0)

`src/resolve/` implements on-demand local module resolution, exported
class/interface DTO extraction, and static Zod schema extraction. It sits
between Controller AST analysis and `BodyShape` creation and does not
change the Route IR / Bruno boundary:

```text
Controller AST analysis (unchanged discovery; Controller candidates only)
    → same-file DTO extraction (existing, unchanged)
    → for routes still Unknown with @Body params:
        on-demand local dependency resolution (src/resolve/)
        → exported class/interface or supported Zod schema properties
        → BodyShape::Fields (≤ 64 properties) or stay Unknown
    → existing Bruno generation and reconciliation
```

Guarantees (all pinned by tests):

- Resolve only dependencies reachable from imports of discovered
  Controllers; never a full TypeScript source scan; never `node_modules`.
- Local relative imports and the limited `tsconfig` `baseUrl` / `paths`
  subset (single-wildcard patterns); implicit `.ts` and `index.ts`.
- Four re-export hops maximum; explicit `(module, symbol)` cycle state;
  enforced budgets: 512 dependency files, 4 MiB aggregate source per run,
  256 KiB per file (`MAX_IMPORT_EDGES` is declared but not enforced;
  `MAX_DTO_DECLARATIONS` warns only — see KNOWN_ISSUES).
- Exported declarations only, resolved through real exported bindings;
  aliased re-exports map public names to source names.
- Conservative Zod subset; `Uncertain` → `BodyShape::Unknown` (a partial
  property list is never published as complete).

Known deferred edges (accepted, tracked in KNOWN_ISSUES.md): lexical-only
path containment (no canonical symlink validation), per-route resolver
caching (shared dependencies re-read), Watch Mode does not invalidate on
dependency-only changes, Staged Sync resolves same-file DTOs only. User-facing
behavior is documented in `website/guide/dto-resolution.md`; the engineering
record is `docs/phases/phase-11-cross-file-dto-resolution.md`.
