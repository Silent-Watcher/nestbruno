# Agent Skill

NestBruno ships an installable **Agent Skill** — a portable instruction
package that teaches AI coding agents how to operate NestBruno correctly.
Install it once, ask your agent to document your NestJS API, and the agent
knows which commands to run, how to read the machine-readable JSON output,
what the analysis can and cannot prove, and where the safety boundaries
are.

The skill follows the open [Agent Skills](https://skills.sh) specification
and is installed with the Skills CLI, which detects the coding agents on
your machine automatically; see the Skills CLI documentation for the
current list of supported agents.

## The problem it solves

A coding agent asked to "document my API" has only two options without
help: guess from decorators (incomplete and often wrong) or boot the
application (slow, and impossible in many environments). The skill gives
the agent a third option — the right one:

- NestBruno **statically** extracts the real route inventory: controllers,
  methods, effective paths, parameters, DTO-derived request bodies.
- The agent reads verified JSON output instead of parsing human terminal
  text or inventing behavior.
- The agent knows the documented limits (no response schemas, no
  validation rules, no runtime authentication) and marks what is unknown
  instead of fabricating it.

## What the skill teaches

- What NestBruno does and when it is the right tool.
- The difference between the **Bruno collection** (machine-usable `.bru`
  files) and **API documentation** (Markdown an agent writes) — and that
  generating the first is never a side effect of asking for the second.
- The correct command for each job and the exact JSON contract of
  `routes`, `discover`, `explain`, `verify` and `sync --dry-run`.
- How to interpret `complete` and `diagnostics`, and that an exit code 4
  means *unknown*, not empty.
- A six-step documentation workflow: inspect → extract → analyze →
  read source → write → validate.
- The accuracy rules: never invent API behavior, never present an
  incomplete analysis as a complete inventory, never expose secrets.
- Safe collection handling: inspect freely, run `verify` or
  `sync --dry-run` first, and only mutate the collection when
  synchronization is actually requested.

## Commands the agent uses

| Command | Purpose |
| --- | --- |
| `nestbruno routes --format json` | The route inventory: controller, handler, method, path, parameters, body shape, source location |
| `nestbruno explain "GET /users/:id" --format json` | How one route was identified and resolved, including path composition and limitations |
| `nestbruno verify --format json` | Whether the collection matches the sources (read-only) |
| `nestbruno sync --dry-run --format json` | What a synchronization would change (read-only) |
| `nestbruno discover --format json` | Candidate controller files |
| `nestbruno check` | Validate an existing configuration |
| `nestbruno sync` / `generate` / `watch` | Collection-mutating operations — used only on request |

The full command reference, including exit codes and the JSON contract,
is in [Commands](/guide/commands).

## How the agent documents an API

1. **Inspect** the project: NestJS root, `nestbruno.toml`, CLI version,
   existing documentation conventions.
2. **Extract** the route inventory with `routes --format json`.
3. **Analyze** individual routes with `explain` where more detail helps.
4. **Read the source** for what static analysis cannot provide —
   descriptions, response shapes, guards, business rules.
5. **Write or update** the documentation in the project's existing
   structure, organized by module or controller for larger APIs.
6. **Validate** everything against the inventory, the `explain` output
   and the actual declarations — and say so when the analysis was
   incomplete.

## Installation

Copy the command below into your terminal and run it inside your NestJS
project (or use your coding agent's skill-install flow):

<AgentSkillInstall />

In plain text:

```bash
npx skills add Silent-Watcher/nestbruno --skill nestbruno
```

The `--skill nestbruno` flag installs this one skill from the
[repository](https://github.com/Silent-Watcher/nestbruno) rather than
every skill it contains. NestBruno is not published as an npm package —
the repository itself is the distribution source, so what you install is
always what the repository ships.

### Project-scoped installation (default)

Without extra flags the skill is installed **project-scoped**: it becomes
available to your coding agents in the current repository only. That is
the recommended setup — the skill activates exactly where you work on the
NestJS project it documents.

### Global installation

To make the skill available in every project, append `-g`:

```bash
npx skills add Silent-Watcher/nestbruno --skill nestbruno -g
```

## Example prompts

After installing the skill, prompts like these produce grounded results:

```text
Document my NestJS API using NestBruno. Organize the documentation by
module and include request parameters, DTO information, and source
references.
```

```text
Inspect my current NestBruno route inventory and update the existing API
documentation. Preserve all manually written descriptions.
```

```text
Check whether my Bruno collection is still in sync with the sources, and
report what would change — do not synchronize.
```

```text
Explain how NestBruno resolves POST /users, including how the path was
composed and what the request body skeleton looks like.
```

## Updating the installed skill

The skill's content is versioned with this repository. To update an
installed copy, re-run the installation command — the Skills CLI
refreshes the skill to the current repository content. Project-scoped and
global installations are updated the same way, with the same command used
to install them.

## Limitations and safety

The skill is explicit about what NestBruno can prove, and your agent
should be too:

- NestBruno performs **static analysis only**; it never executes your
  application. Dynamically constructed routes and unprovable bootstrap
  configuration may remain unresolved.
- Global prefixes and URI versioning appear only when statically
  provable ([Prefix & Versioning](/guide/routes)).
- DTO resolution has explicit boundaries and budgets
  ([DTO Resolution](/guide/dto-resolution)); extracted fields are names
  only — never types, defaults or validation rules.
- Response schemas, authentication requirements, guards and business
  logic are not inferred. The agent must find them in the source or leave
  them out.
- An incomplete analysis (exit code 4) means the affected results are
  *unknown* — it must never be presented as a complete inventory.
- Inspection commands are safe for any task; `sync`, `generate`, `watch`
  and `init` modify files and are used only when synchronization is the
  actual request. User-owned and hand-written collection files are
  preserved and reported as conflicts, never silently overwritten
  ([Safety & Ownership](/guide/safety)).

The agent is also instructed never to read `.env` files for values and
never to place credentials in generated documentation.
