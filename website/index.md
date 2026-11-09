---
layout: home

hero:
  name: "NestBruno"
  text: "Your NestJS API, always in sync with Bruno"
  tagline: "A Rust CLI that reads your controllers and their imported DTOs — statically, without ever running your application — and keeps a Bruno API collection synchronized with the API you are actually building."
  image:
    src: /logo.svg
    alt: NestBruno logo
  actions:
    - theme: brand
      text: Get Started
      link: /guide/getting-started
    - theme: alt
      text: How DTO resolution works
      link: /guide/dto-resolution
    - theme: alt
      text: GitHub
      link: https://github.com/Silent-Watcher/nestbruno

features:
  - icon: 🔍
    title: Understands more than routes
    details: NestBruno follows local TypeScript imports, resolves DTO classes, interfaces and a conservative Zod subset in separate files, and generates complete JSON body skeletons — without executing anything.
  - icon: 🪄
    title: Static, safe analysis
    details: Your TypeScript is parsed with a real AST parser (oxc) — never executed. No Node, no network, no arbitrary code.
  - icon: 🔁
    title: Incremental sync
    details: Changed a controller? Only that file is re-parsed and only its requests are reconciled. Watch mode keeps it live while you code.
  - icon: 🛡️
    title: Your edits are never clobbered
    details: Every generated request carries an ownership stamp. Bruno files you touched are locked and reported as conflicts, never silently overwritten.
  - icon: 🧭
    title: Deterministic output
    details: The same source produces byte-identical .bru files — clean Git diffs, reviewable changes, no churn.
  - icon: 🧪
    title: 535 tests, property-tested
    details: Reconciliation invariants, hostile inputs, symlink attacks, circular imports and malformed source are covered by the suite that gates every commit.
---

<div class="vp-doc nb-quickstart">

## It follows your imports

A `@Body()` parameter is just a type name — unless your tool understands
what that name points to. NestBruno resolves DTO declarations in separate
files and turns them into usable request bodies:

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

Generated Bruno request — body included:

```json
{
  "name": null,
  "email": null,
  "password": null
}
```

```text
Controller ──▶ imported DTO ──▶ property extraction ──▶ JSON skeleton ──▶ Bruno request
```

Class DTOs and [Zod schemas](/guide/dto-resolution#zod-schemas) (`z.object`,
`z.infer`, `createZodDto`, statically provable transformations) are both
supported — through relative imports, barrels and re-exports, bounded to
four hops. Anything NestBruno cannot resolve confidently stays an empty
body, never a guessed one.

## Measured, not marketed

Component benchmarks from the shipped Criterion suite (release build;
i7-1255U, Linux; see [methodology](/guide/benchmarks)):

<div class="nb-stats">
  <div class="nb-stat">
    <div class="nb-value">~40 µs</div>
    <div class="nb-label">resolve a controller's imported DTO</div>
  </div>
  <div class="nb-stat">
    <div class="nb-value">~540 µs</div>
    <div class="nb-label">extract 100 controllers / 300 routes</div>
  </div>
  <div class="nb-stat">
    <div class="nb-value">~12 ms</div>
    <div class="nb-label">discover 1,000 files</div>
  </div>
  <div class="nb-stat">
    <div class="nb-value">~13 ms</div>
    <div class="nb-label">no-op sync of a 750-request collection</div>
  </div>
</div>

<p class="nb-note">
Component benchmarks, not full-pipeline timings; absolute values vary with
hardware. Reproduce with <code>cargo bench</code>.
</p>

## Two commands to a living collection

<div class="nb-terminal">

```sh
nestbruno init     # write nestbruno.toml with sane defaults
nestbruno watch    # generate the collection, then keep it in sync
```

</div>

Edit `src/users/users.controller.ts`:

```ts
@Controller('users')
export class UsersController {
  @Get(':id')
  findOne(@Param('id') id: string) {}

  @Post()
  create(@Body() dto: CreateUserDto) {}
}
```

…and the collection updates itself:

```txt
Changed: src/users/users.controller.ts
  Routes affected: 2
  Created: 1  Updated: 1  Deleted: 0  Conflicts: 0
```

## What lands in your repo

```txt
bruno/
├── bruno.json
└── Users/
    ├── find-one.bru      # GET {{baseUrl}}/users/:id   + path params
    └── create.bru        # POST {{baseUrl}}/users      + JSON body skeleton
```

Every `.bru` file is valid Bruno — generated files round-trip through
Bruno's own parser — and ends with a tool stamp in its docs block, which is
how NestBruno knows what it owns and what you own. Hand-written requests
and files you edited are preserved and reported as conflicts, never
overwritten.

</div>
