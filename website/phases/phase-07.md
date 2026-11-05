# Phase 7 — Staged Synchronization

**Ships:** `nestbruno sync --staged [--dry-run]` — reconcile the collection
with the state the Git **index** represents: exactly what you have staged,
not what is lying around in the working tree.

## How it works

```text
git index (+ HEAD) ─▶ staged-change gate ─▶ staged content of every relevant file
                                   │
              in-memory analysis ─▶ the same reconciler as sync
```

- **The index is the truth.** A staged file is analyzed from its staged
  content — never the working-tree version. Edit a file after staging and
  `sync --staged` still sees what you staged.
- **The gate:** nothing happens unless a staged change touches a relevant
  controller file (`No relevant staged NestJS changes detected.`). Unrelated
  staged files (README, lockfiles, `node_modules`) never trigger work.
- **The state:** when there is staged work, the desired routes come from the
  **whole index**, not a patchwork — the same invariant as always:
  *staged sync ≡ a fresh sync of the staged project state* (tested
  byte-for-byte).

## What each staged change does

| Change | Behavior |
| --- | --- |
| Added file | requests created |
| Modified file | requests updated in place |
| Deleted file | its unmodified requests deleted (your edits stay conflicts) |
| Renamed file | requests updated **in place** — no delete/create cycle (rename detection + identity fingerprints) |

All Phase 5 ownership rules apply unchanged: your edited requests are
conflicts, unknown files are preserved, and nothing is deleted while any
staged file fails to analyze (merge conflicts included).

## Safety

- **Git is never touched**: the index, HEAD and branches are only read.
  libgit2 is linked in (no `git` process, no hooks, no shell).
- A repository without commits works: everything staged is measured against
  an empty tree, so `git init && git add . && nestbruno sync --staged` is a
  supported first sync.
- Projects nested inside a worktree are supported; entries outside the
  project root are ignored.
- The index is snapshotted once: a concurrent `git add` during the run is
  picked up by the next run, never mixed into this one.

::: tip Full record
Decisions, error policy, path handling and the complete test inventory:
`docs/phases/phase-07.md` in the repository.
:::
