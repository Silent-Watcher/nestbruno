# DECISIONS.md — architectural decision index

Full rationale lives in the linked phase documents; this is the index an
agent must consult before changing any of these areas. Reversing a decision
requires a concrete reason recorded next to the phase doc.

| # | Decision | Where | Why it exists |
| --- | --- | --- | --- |
| 1 | Library crate holds all behavior; `main.rs` is a thin binary | phase-01 | every path testable without spawning a process |
| 2 | `nestbruno.toml`: all keys optional, `deny_unknown_fields`, size cap 1 MiB | phase-01, `src/config.rs` | zero-config start; typos are errors not defaults |
| 3 | Discovery is path-only ("Option A"); `@Controller` verification deferred to AST analysis | phase-02 | no file reads during discovery; clean reuse by watch |
| 4 | oxc 0.116 as the single TypeScript parser (one release train; pins Rust 1.91) | phase-03, `Cargo.toml` | mature, fast, no regex parsing, no execution |
| 5 | Syntax error ⇒ **no routes** from that file (recovered AST never used) | phase-03 | never silently drop routes from a half-parsed file |
| 6 | Route IR is the only parser↔Bruno boundary; generator never reads source or `.bru` | phase-03/04 | enables incremental, watch, and future outputs (OpenAPI) |
| 7 | One folder per controller, one `.bru` per route; kebab-case; `-<8 hex>` collision suffixes; Windows reserved names | phase-04, `bruno/naming.rs` | deterministic, collision-safe, readable |
| 8 | Route identity = FNV-1a 64 over controller+method+path+handler, in three tiers (`id`/`key`/`anchor`) | phase-04/05, `bruno/identity.rs` | renames/moves/update-in-place without UUIDs or timestamps |
| 9 | Ownership stamp lives in the `docs` block (Bruno drops unknown `meta` keys, keeps `docs`) | phase-05, `bruno/stamp.rs` | survives Bruno round-trips; user-visible but claim-verified |
| 10 | Reconciler is pure; apply re-verifies every hash and path before writing | phase-05 | plan-then-write safety; dry-run is the same plan |
| 11 | Critical conflicts (duplicate identities) block **all** writes | phase-05 | never half-apply an ambiguous collection |
| 12 | Deletions only when analysis is complete (`is_incomplete()` gate) | phase-05/06 | broken source must never cause destructive sync |
| 13 | Watch events are hints (dirty set + re-stat), with full-resync fallbacks | phase-06, `workspace.rs` | event order/rename ambiguity cannot corrupt state |
| 14 | Watch keeps last-good routes for files that fail to parse | phase-06 | editors save transiently broken files |
| 15 | `bruno.json` is created once, never replaced, hardcoded `"API"`/`"collection"` version 1 | phase-04, `bruno/writer.rs` | tool never clobbers collection metadata |
| 16 | `sync` re-reads all `.bru` per run; watch keeps an in-memory index (no persistent cache anywhere) | phase-05/06 | simplicity; persistent caching deliberately deferred |
| 17 | Exit codes 0/1/2/3/4; "incomplete" (4) is an `Outcome`, not an `Error` | phase-01, `error.rs` | recoverable per-path problems are not fatal |
| 18 | Watch e2e tests are Unix-only, wait on output lines instead of sleeps | phase-06 | determinism without timing flakiness |
| 19 | Git access via `git2` 0.21 (libgit2, linked), never shelling out | phase-07, `staged.rs` | index/HEAD/blob + rename detection; no external `git`, no hooks, no subprocess |
| 20 | `sync --staged` desired state is always the full index (gate on staged changes, but no incremental patching) | phase-07 | staged sync ≡ fresh sync of the staged state, tested byte-for-byte; avoids route models matching neither state |

| 21 | Cross-file DTO dependencies are resolved on demand from Controller imports; Controller discovery remains path-only and unchanged | Phase 11 handoff | avoids project-wide TypeScript scans and keeps discovery semantics stable |
| 22 | Local import resolution only: relative paths plus limited `baseUrl` / `paths`; no package or `node_modules` traversal | Phase 11 handoff | bounds filesystem access and avoids pretending to be a full TS resolver |
| 23 | Re-export traversal is capped at four hops and uses explicit cycle/symbol state | Phase 11 handoff | deterministic, bounded graph resolution |
| 24 | Class/interface DTOs and statically recognized Zod schemas share the existing `BodyShape` IR | Phase 11 handoff | keeps generation independent of TypeScript and Zod |
| 25 | Unsupported, ambiguous, malformed, or uncertain DTO metadata stays `BodyShape::Unknown` | Phase 11 handoff | never generate a guessed or partial request-body shape |
| 26 | Dependency reads are gated by symlink-free, regular-file containment against the canonical project root (`resolve/containment.rs`); candidate selection never follows links | Phase 11.1, Milestone 11.1.1 | untrusted trees must not redirect dependency reads through symlinks; special files must never be opened (a FIFO would block the analyzer) |
| 27 | All resolver budgets share reject-the-exceeding-operation semantics; `MAX_IMPORT_EDGES` and `MAX_DTO_DECLARATIONS` are enforced, not advisory | Phase 11.1, `resolve/limits.rs` | no partially tracked state is ever published; budget exhaustion degrades to `BodyShape::Unknown` |

## Deliberately *not* decided / open

- **Benchmark extensions for Phase 11**: dimensions and reporting are to be
  selected after correctness and measured against the existing Criterion suite.
- License: MIT (chosen by the owner 2026-11-06; `LICENSE` +
  `license = "MIT"`).
