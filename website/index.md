---
layout: home

hero:
  name: "NestBruno"
  text: "Your NestJS API, always in sync with Bruno"
  tagline: "A Rust CLI that keeps a Bruno API collection synchronized with your controllers' HTTP routes — statically, without ever running your application."
  image:
    src: /logo.svg
    alt: NestBruno logo
  actions:
    - theme: brand
      text: Get Started
      link: /guide/getting-started
    - theme: alt
      text: What it generates
      link: /guide/

features:
  - icon: 🪄
    title: Static, safe analysis
    details: Your TypeScript is parsed with a real AST parser — never executed. No Node, no network, no surprises.
  - icon: 🔁
    title: Incremental sync
    details: Changed a controller? Only that file is re-parsed and only its requests are reconciled. Watch mode keeps it live while you code.
  - icon: 🛡️
    title: Your edits are never clobbered
    details: Every generated request carries an ownership stamp. Bruno files you touched are locked and reported as conflicts, never silently overwritten.
  - icon: 🧭
    title: Deterministic output
    details: The same source produces byte-identical .bru files — clean Git diffs, reviewable changes, no churn.
  - icon: 🚀
    title: Built for big repos
    details: Path-only discovery, bounded reads, property-tested reconciliation. Tested against 10k-file trees and 5k-request collections.
  - icon: 🧪
    title: 443 tests, 17 property suites
    details: Reconciliation invariants, hostile inputs, symlink attacks and malformed source are covered by the suite that gates every commit.
---

<div class="vp-doc nb-quickstart">

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

Every `.bru` file is valid Bruno — generated files round-trip through Bruno's
own parser — and ends with a tool stamp in its docs block, which is how
NestBruno knows what it owns and what you own.

</div>
