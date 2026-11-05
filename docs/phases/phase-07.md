# Phase 7: Git staged synchronization

## Objective

`nestbruno sync --staged`: synchronize the collection with the state the Git
**index** represents — the staged project state — instead of the working
tree, so requests can be reconciled for exactly what is about to be
committed.

```text
git index (+ HEAD) ─▶ staged changes (the gate) ─▶ staged sources (the state)
                                   │
        in-memory content ─▶ analyze_sources ─▶ the Phase 5 reconciler, unchanged
```

## Scope

Implemented: repository detection, index snapshot, staged-change gate,
staged-content analysis (added / modified / deleted / renamed /
type-changed), staged desired-state reconstruction, dry-run, structured Git
errors, tests, docs. Not implemented: any Git mutation (the index, HEAD,
branches are never touched), hooks, CI integration, incremental staged
caching.

## Architecture

| Module | Role |
| --- | --- |
| `staged.rs` | The Git boundary: `staged_sources(project, matcher) -> Staged`. The only place that touches libgit2. |
| `nest/mod.rs` | `analyze_sources(&[(PathBuf, String)])`: analysis of in-memory content on the same big-stack worker; `analyze_each` refactored to share the pipeline. |
| `commands.rs` | `sync --staged [--dry-run]`: gate → analyze → generate → the same `synchronize`. |
| `sync/*` | Unchanged: the same reconciler, writer and conflict rules. |

`Staged { changes, sources, diagnostics }`:
- `changes` — the *gate*: staged deltas whose paths pass the Phase 2
  `Matcher` (relative to `source`). Empty ⇒ the command prints
  `No relevant staged NestJS changes detected.` and does nothing (exit 0).
- `sources` — the *state*: the staged content of **every** relevant index
  entry, not just changed ones. Reconciliation needs the complete desired
  set to detect stale requests; patching a working-tree analysis with staged
  files could produce a route model matching neither state, so the desired
  state is always reconstructed purely from the index.

## Git access: a library, not a subprocess

`git2` 0.21 (libgit2, linked in). Reasons: the prompt requires
index/HEAD/blob access with rename detection; `std` has none; shelling out
would couple the tool to an installed `git` and its configuration, and is
the pattern the security requirements warn against. libgit2 never runs
hooks, never spawns processes, and reads only `.git`. The same crate builds
the temporary-repository test fixtures (it is also a dev-dependency).

## Staged-state semantics

- **The index is the truth.** A staged file is analyzed from its index
  blob; the working-tree version is never read. A file with unstaged-only
  changes does not participate at all (and with no staged relevant changes
  the command is a clean no-op).
- **Deletions**: a path in HEAD but not in the index disappears from the
  desired state, so its unmodified requests are deleted (gated on
  completeness, as everywhere).
- **Renames**: staged renames (old path removed, new added) are detected
  via `diff.find_similar`. No special reconciliation case is needed — the
  Phase 5 fingerprints (`key`, `anchor`) update existing requests in place.
- **Unborn HEAD** (`git init` without commits): the diff runs against an
  empty tree, so the initial `git add . && nestbruno sync --staged` is the
  supported first-sync workflow.
- **Nested project roots**: the project may live inside the worktree;
  worktree-relative index paths are mapped into the project by stripping
  the prefix, and entries outside the project root are ignored entirely.
- **Conflicts**: an unmerged path has no stage-0 entry; it is reported as
  an error diagnostic ("staged content is unknown") and excluded, which
  marks the run incomplete — so nothing is deleted while a merge conflict
  is staged.
- **Snapshot semantics**: the index is read once at the start; a
  concurrent `git add` during the run is picked up by the next run, never
  mixed into this one. No transactional snapshot is pretended.
- **Config staged changes** are deliberately out of scope: the working-tree
  `nestbruno.toml` is used, and the limitation is documented. Staged
  configuration is indistinguishable from working-tree configuration for
  route generation in every supported workflow, because only `source` /
  `bruno` / include-exclude affect which files exist — and those come from
  the index regardless.

## Path handling

Index paths are raw bytes; entries with non-UTF-8 paths are ignored (a
verbose note, like non-UTF-8 file names elsewhere — they never make the
result "incomplete"). Mapped paths must consist of normal components only
(absolute, `..`, and empty paths are ignored with a note). The configured
`source` is matched by components, so `.` and `./src` behave exactly as in
discovery, and paths outside the source directory cannot become relevant.
Every staged path is project-relative in the same form `discover`
produces, which is what keeps the identity fingerprints — and therefore
request matching — identical to the working-tree pipeline.

## Error handling

| Situation | Policy |
| --- | --- |
| No repository at or above the project root | fatal `Error::GitNotARepository` (exit 1) |
| Bare repository / unreadable index / unreadable HEAD / project root outside the worktree | fatal `Error::GitRepository` (exit 1) |
| Unmerged (conflicted) staged path | `error:` diagnostic, path skipped, exit 4, nothing deleted |
| Blob unreadable, oversized (> 256 KiB) or non-UTF-8 | `error:` diagnostic, path skipped, exit 4 |
| Non-UTF-8 or unsafe index path | `note:` (verbose only), ignored |
| Relevant staged change + dry run | plan printed, nothing written |
| No relevant staged change | `No relevant staged NestJS changes detected.`, exit 0 |

Output: `Staged: N relevant change(s):` with one `<status> <path>` line per
change, then the ordinary sync report.

## Security

- The repository is untrusted input. libgit2 runs no hooks and no
  processes; no shell exists anywhere in the pipeline.
- Paths from the index are validated (plain relative components) and
  mapped into the project before use; content is size-capped and
  UTF-8-checked exactly like files read from disk.
- Nothing in the Git boundary writes: not the index, not the worktree, not
  the collection (only `sync`'s apply path writes, as before).
- The parser consumes staged content through the same per-file panic guard
  and stack bounds as disk content.

## Performance

- The index replaces the filesystem walk: no `discover` pass, and blob
  content is read only for relevant paths (filtered by the same globs as
  discovery).
- When nothing relevant is staged, the run costs one index read plus one
  diff — no parsing, no collection read.
- Otherwise the cost is that of a full `sync` over the index state (one
  parse per relevant file), which is the price of the correctness
  invariant. No caching is added.

## Testing

- Unit (`src/staged.rs` tests, real libgit2 fixtures in temp dirs): empty
  tree vs unborn HEAD; staged content used over the working tree; no-op
  when the index matches HEAD; deletion and rename detection; filtering of
  unrelated/pruned/out-of-source entries; nested project-root mapping;
  non-UTF-8 and oversized blobs; structured no-repository error;
  `is_plain`/`source_relative` mapping (`.` and prefixes).
- Integration (`tests/staged.rs`, real binary): clear error without a
  repository; clean no-op for unrelated staged files (collection not even
  created); create; staged-version-not-worktree; unstaged ignored; staged
  deletion deletes the unmodified request and empties the folder; staged
  rename updates in place (no create/delete cycle); user-modified request
  conflicts (exit 4, edit preserved); dry run writes nothing and predicts
  the real run; idempotent second run; Unicode/space paths; nested project
  root; **staged sync ≡ fresh sync of the materialized staged state**
  (byte-identical collections); an unparsable staged file keeps other
  files syncing and suppresses deletions.
- All 470 tests pass (443 from Phases 1–6 + 27 new); fmt and clippy
  (`-D warnings`) pass.

## Limitations

- The desired state is always the full index (no incremental staged
  analysis): correct by construction, linear in the project size.
- Only relevant *tracked-by-the-index* files exist for staged sync;
  untracked files are invisible to it by definition.
- Staged configuration changes are not modeled (see above).
- No `Copied` status (libgit2 reports copies as additions; the status enum
  documents this).
- A concurrent index change during the run is picked up on the next run
  (snapshot semantics), and a merge with staged conflicts yields an
  incomplete run rather than a best-effort one.
- `sync --staged` and `sync` overlap by design; use `--staged` when the
  staged state should win.

## Acceptance criteria

All Phase 7 prompt items are met within the limitations above: index as
source of truth, staged-content parsing, added/modified/deleted/renamed
handling, selective gate with a full-state fallback (by construction),
dry-run, structured Git errors, path normalization layer, unrelated-file
filtering, empty-state no-op, working-tree safety (no Git mutation), and
the equivalence invariant, verified by the integration test.

## Commits

- `feat: add staged git synchronization`
- `test: add staged synchronization integration tests`
- `docs: document phase 7 staged synchronization`

## What Phase 8 builds on

The hardening phase can benchmark `sync --staged` against large indexes,
extend the exit-code contract documentation, and reuse the Git boundary
for any further selective workflows.
