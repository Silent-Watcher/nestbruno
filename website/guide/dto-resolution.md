# DTO Resolution

A `@Body()` parameter typed with a DTO class is the interesting case for
request generation: the type name alone tells Bruno nothing. NestBruno
resolves these declarations statically — in the same file **and across
local imports** — and turns the property names into a JSON body skeleton.

```text
Controller                dto/create-user.dto.ts
   │
   │  import { CreateUserDto } from './dto/create-user.dto';
   ▼
Import resolution ──▶ module: dto/create-user.dto.ts
   ▼
Property extraction ──▶ name, email, password
   ▼
BodyShape::Fields ──▶ Bruno JSON skeleton
```

Every generated value is `null`. The skeleton is a starting point for
manual editing — it is **not** a validation schema, and no runtime type
information is inferred.

## The three body shapes

Every route body is classified into exactly one of:

| Shape | Meaning | Generated output |
| --- | --- | --- |
| `None` | No `@Body()` parameter | `body: none` |
| `Unknown` | A body exists but the type could not be resolved confidently | `{}` (empty JSON body) |
| `Fields` | Property names resolved with confidence | `{"name": null, ...}` — one key per property, ≤ 64 |

`Unknown` is the honest fallback. When an import cannot be resolved, an
export is ambiguous, or a Zod transformation cannot be analyzed, NestBruno
does not invent a partial field list — it emits an empty body and, where
the reason is identifiable, a warning diagnostic.

## Same-file DTOs

Top-level `class` and `interface` declarations in the controller's own file
are collected directly — property names in source order, computed keys
skipped, methods and constructors ignored:

```ts
class CreateUserDto {          // same file as the controller
  name: string;
  email: string;
  password: string;
}
```

::: warning Same-file declarations must not use `export`
The same-file extractor recognizes bare `class` / `interface` declarations
only. A **`export class CreateUserDto {}` in the controller's own file is
currently not picked up** and the body stays `Unknown`. Exported DTOs
belong in their own module (see below) — which is the conventional NestJS
layout anyway.
:::

At most **64 properties** per DTO are used (source order); a larger class
contributes its first 64. The skeleton is a placeholder, not a schema, and
pathological classes must not produce pathological output.

## Cross-file DTOs

When a `@Body()` type is not resolved from the same file, NestBruno follows
its import. Supported module forms:

- relative imports: `./dto/create-user.dto`, `../shared/dto`, with or
  without the `.ts` extension;
- NodeNext-style emitted extensions in specifiers: `./dto/create-user.dto.js`
  resolves to `create-user.dto.ts` (likewise `.mjs` → `.mts`, `.cjs` →
  `.cts`) — the form `moduleResolution: "NodeNext"` requires;
- directory imports resolved through `index.ts`;
- `tsconfig.json` `baseUrl` and `paths` — patterns with exactly one `*`
  (e.g. `"@dto/*": ["src/dto/*"]`); the first target of each mapping is
  used, and targets are interpreted relative to the project root.

```ts
// users.controller.ts
import { CreateUserDto } from './dto/create-user.dto';

@Controller('users')
export class UsersController {
  @Post()
  create(@Body() dto: CreateUserDto) {}
}
```

```ts
// dto/create-user.dto.ts
export class CreateUserDto {
  name: string;
  email: string;
  password: string;
}
```

Generated body:

```json
{
  "name": null,
  "email": null,
  "password": null
}
```

The declaration must actually be **exported** by the module it is imported
from. Exported classes and interfaces are supported; named and type-only
imports, aliased imports, and re-exports all work.

### Re-exports and barrels

Re-export chains are followed up to **four hops**:

```ts
// barrel.ts
export { CreateUserDto } from './create-user.dto';
export { AuditDto as TraceDto } from './audit.dto';
export * from './common.dto';
```

Exceeding the hop limit, or a cycle among re-exports, yields `Unknown`
plus a warning diagnostic. `node_modules` is never traversed — a bare
specifier such as `zod` only resolves through a `paths`/`baseUrl` mapping.

### Zod schemas

NestBruno recognizes a **conservative subset** of Zod-backed DTOs —
schemas from the `zod` package and DTO classes built with
[`nestjs-zod`](https://www.npmjs.com/package/nestjs-zod)'s
`createZodDto`:

```ts
// schemas/pagination.ts
import { createZodDto } from 'nestjs-zod';
import { z } from 'zod';

const baseSchema = z.object({ sort: z.string(), filter: z.string() });
export const sortSchema = baseSchema.pick({ sort: true });

export class SortDto extends createZodDto(sortSchema) {}
```

```ts
// users.controller.ts
import { SortDto } from './schemas/sort.dto';

@Post('sort')
sort(@Body() dto: SortDto) {}
// → body: { "sort": null }
```

Supported:

- `z.object({ ... })` with static keys (identifiers, string and numeric
  literals); computed keys are skipped;
- namespace (`import * as z`), named (`import { object }`) and aliased
  imports of `zod`;
- `z.infer<typeof Schema>` type aliases — the `z` qualifier is verified to
  come from a real `zod` import; the schema may be declared **later in the
  same file**, or **imported/re-exported from another module**;
- `createZodDto(...)` classes **defined in an imported module**, where the
  argument is an inline `z.object(...)`, a schema constant in the same
  file, or a schema imported into that file; the import may be named,
  aliased, or a namespace (`nz.createZodDto`);
- **multi-step transformation chains** whose base is an inline
  `z.object({ ... })` or a schema constant — `extend`, `pick`, `omit`,
  `partial`, `required`, `strict`, `passthrough`, `strip` applied in
  source order, only when the resulting property set can be established
  statically (e.g. `pick`/`omit` require literal `true` values);
- `package.json` `imports` subpaths: local `#src/...` specifiers resolve
  through exact and single-`*` wildcard mappings, read through the same
  source provider as every dependency (staged analysis sees the staged
  `package.json`). External targets are never resolved.

::: warning Zod boundaries
`createZodDto` requires a real `nestjs-zod` import — a bare call is not
recognized. DTO classes extending `createZodDto` **in the controller's own
file** are not resolved (same-file extraction collects property
declarations, not `extends` clauses). Chains starting at something other
than `z.object` or a schema constant (e.g. `z.string().transform(...)`),
an unknown method, a non-literal `pick` value, an unresolvable schema —
anything uncertain stays `Unknown`. This is deliberately not comprehensive
Zod compatibility.
:::

## Where resolution runs

Cross-file resolution is part of on-disk analysis: `generate`, `sync`,
`watch` and `routes` all resolve imported DTOs. `sync --staged` resolves
them too, against the Git index snapshot itself (added 2026-11-10): a
dedicated source provider reads dependency content from the captured
stage-0 blobs, so index and working-tree state are never mixed — a staged
controller resolves against the staged DTO, even when the working tree
holds something else. A staged `tsconfig.json` supplies `baseUrl`/`paths`
mappings; symlink or non-regular index entries are rejected and degrade to
the conservative unknown shape.

## Query and param DTOs

Unnamed `@Query()` and `@Param()` object parameters resolve through the
same pipeline (same-file, cross-file, Zod, re-exports — everything above):

- `@Query() query: FindUsersQueryDto` — the DTO's top-level properties
  become **disabled, empty** query parameters (visible in Bruno, not sent),
  deduplicated against named `@Query('page')` parameters.
- `@Param() params: FindPostParamsDto` — path parameters keep coming from
  the URL. DTO fields are matched against them; a DTO field that is not in
  the path is documented on the request, and no placeholder is ever
  invented.
- Named `@Query('x') x` / `@Param('id') id` stay single named parameters —
  never flattened.

```ts
@Get()
findAll(@Query() query: FindUsersQueryDto) {}
// → params:query { ~page:  ~limit:  ~search: } (disabled, empty)
```

## Resource bounds

Dependency traversal is bounded by internal constants (not configuration):
at most four re-export hops, 512 dependency files and 4 MiB of aggregate
dependency source per run, 256 KiB per file, 1024 DTO declarations, 64
properties per DTO. A pathological import graph degrades to `Unknown`
bodies with warnings — never to unbounded work. See
[Safety](/guide/safety) for the filesystem rules that apply to imported
files.
