# Phase 2 — Controller Discovery

**Ships:** `nestbruno discover` — fast, deterministic discovery of the
TypeScript files that *may* contain controllers, from paths alone.

## How it works

```text
compile include/exclude globs (once)
  → resolve source root, verify it is inside the project
  → iterative walk: skip .git / node_modules / excluded dirs / depth > 64
  → file filter: .ts (not .d.ts), included and not excluded
  → sort → deterministic candidate list
```

Discovery never reads file contents — a candidate check is one `read_dir`
per directory. Whether a candidate really contains `@Controller` is decided
later by the AST analysis (Phase 3), the only component that parses
TypeScript.

## Semantics worth knowing

- Patterns match **relative to `source`**; `*` doesn't cross `/`, `**` does;
  exclusion wins; a matched directory is not entered.
- Because matching is relative to `source`, a project under a directory
  named `dist` is not accidentally ignored — only *nested* excludes are.
- `.git` / `node_modules` are always skipped; `.d.ts` / `.tsx` / `.js` are
  not candidates.
- Symlinks inside the tree are never followed (loops terminate); the source
  root itself may be a symlink only when it resolves inside the project.
- One unreadable directory marks the result **incomplete** (exit 4) instead
  of failing the run — "state unknown", never silently "no controllers".

## Performance

An ad-hoc measurement on a synthetic 100,000-file tree: ~30 ms and ~11 MB
RSS, with `node_modules` (50,000 files) never entered. No cache — that
arrives with watch mode.

::: tip Full record
Error policy table, security details, test inventory:
`docs/phases/phase-02.md` in the repository.
:::
