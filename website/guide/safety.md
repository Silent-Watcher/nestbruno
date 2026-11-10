# Safety & Ownership

NestBruno treats your repository as **untrusted input**. These are the
guarantees, and how they are enforced.

## Your code is never executed

TypeScript is parsed with a real AST parser ([oxc](https://oxc.rs/)) and
inspected statically. NestBruno never:

- executes JavaScript or TypeScript, evaluates decorators, or imports your
  modules;
- boots NestJS or invokes Node.js;
- runs shell commands or touches the network;
- uses `unsafe` Rust (forbidden crate-wide) or `unwrap` in production code.

A decorator argument it cannot statically understand (a variable, a function
call, concatenation) is reported as *unresolved* with a warning — never
guessed, never evaluated.

## Hostile source cannot corrupt output

- All source-derived text (controller names, handler names, path text) is
  validated before it reaches a `.bru` file: control characters, line
  separators and <span v-pre>`{{`</span> are rejected — a newline in a path could otherwise
  inject a new block, <span v-pre>`{{process.env.X}}`</span> could leak environment variables.
- The serializer independently refuses anything Bruno could misread and
  produces **no file at all** on failure.
- These guards are mutation-tested: removing one makes an injection test
  fail.

## Paths stay inside the collection

- Generated file/folder names come from a fixed ASCII alphabet; the writer
  re-validates every component (no `..`, separators, control characters,
  > 255 bytes) before use.
- The collection directory is validated by the configuration and walked from
  the project root; a symlink anywhere on a write path aborts the write
  instead of following it.
- Discovery never follows symlinks inside the source tree (loops terminate;
  nothing outside the project is read).

## Import analysis boundaries

Cross-file DTO resolution reads additional TypeScript files — the files
your controllers import. This is bounded and conservative, with known
edges:

- Only modules reachable from **imports of discovered controller files**
  are read; the resolver never scans the source tree for DTO files and
  never enters `node_modules`.
- Work is bounded per run: four re-export hops, 4,096 import edges, 512
  dependency files, 4 MiB aggregate dependency source, 256 KiB per file,
  1,024 DTO declarations, 64 properties per DTO. All bounds are enforced
  with reject-the-exceeding-operation semantics: exceeding one yields an
  `Unknown` body with a warning — never unbounded traversal, never a
  partially analyzed module.
- Imported files are subject to the same read rules as controller files
  (`.git` and `node_modules` components are hard-skipped; configured
  exclude patterns apply).
- **Known gap:** module containment is currently checked lexically (no
  `..` that escapes the project), but canonical symlink validation for
  imported files is not yet enforced — a symlink inside the source tree
  could point the resolver at a file outside it. Hardening this is a
  recorded follow-up; nothing is ever executed regardless of what is read.

## Environment files never carry secrets

Generated environment files (`nestbruno generate --environments`) contain
**empty placeholders by construction**: NestBruno reads nothing of your
application — no `.env` files, no runtime values — so no secret can enter
a generated file. A `{{ process.env.X }}` reference is reported as a
warning with its name only. Once an environment file exists it is
user-owned: it is never modified, merged or deleted, not even with
`--force`, so your credentials cannot be overwritten. Committing
environments is your choice; keep real secrets out of Git.

## Your files are never clobbered

- Writes are atomic: content is fully built and validated in memory, then
  written to a temp file in the target directory, `fsync`ed, and renamed
  into place. A failed write leaves the old file; there are no partially
  written `.bru` files.
- Replace and delete re-verify the file's content hash immediately before
  the operation.
- Unknown files (hand-written requests, environments, `folder.bru`),
  modified generated files, and files with invalid metadata are preserved —
  conflicts are reported with a suggested action instead.
- Nothing is deleted while any source file could not be analyzed.
- `bruno.json` is created once and never replaced — not even with
  `--force`.

## Malformed input degrades gracefully

A syntax error in one controller produces one diagnostic with
`file:line:col` and no routes from that file; every other file continues.
The command exits 4 ("incomplete") so scripts can tell a partial result
from a clean one — and sync/watch treat "incomplete" as "may not delete".

## Resource limits

Bounded file reads (sources ≤ 256 KiB, configs ≤ 1 MiB, `.bru` ≤ 4 MiB),
a depth-capped iterative walk, a large-stack worker thread sized against
measured parser stack usage, bounded watcher channels and diagnostic sets.
Pathological repositories are reported, not crashed on.
