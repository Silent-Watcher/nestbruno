# Getting Started

## Requirements

- A NestJS project with TypeScript controllers (`.ts`, not `.d.ts`).
- Rust 1.91+ if installing from source.

## Installation

Install the published crate from [crates.io](https://crates.io/crates/nestbruno):

```sh
cargo install nestbruno
```

Or build from source (requires Rust 1.91+):

```sh
git clone https://github.com/Silent-Watcher/nestbruno
cargo install --path nestbruno
```

### Shell completions (optional)

Generate a completion script for your shell and place it where the shell
finds it — details in [Commands](/guide/commands#nestbruno-completions-shell):

```sh
nestbruno completions bash > ~/.local/share/bash-completion/completions/nestbruno
```

## First collection in three steps

### 1. Initialize

```sh
nestbruno init
```

writes a `nestbruno.toml` with sane defaults and never overwrites an existing
one. Defaults: source `src`, collection `bruno`, include
`**/*.controller.ts`, excludes for `dist`, `build`, `coverage`, `.tmp`,
`.cache`.

Check that the layout is valid:

```sh
nestbruno check
```

### 2. Generate or sync

```sh
nestbruno sync
```

analyzes your controllers and reconciles the `bruno/` collection:
creates requests for new routes, updates the ones it owns that changed,
deletes requests of removed routes **only if you never edited them**, and
preserves everything else.

`sync --dry-run` prints the exact same plan without writing anything — use it
first.

### 3. Open it in Bruno

Open the `bruno/` directory in Bruno. Define `baseUrl` in a Bruno
environment (NestBruno deliberately creates no environment file) and the
requests are ready: URLs with <span v-pre>`{{baseUrl}}`</span>, path parameters listed in
`params:path`, query/header parameters visible but disabled, a JSON body
skeleton for `@Body` handlers.

## A living example

```ts
@Controller('users')
export class UsersController {
  @Get()
  findAll() {}

  @Get(':id')
  findOne(@Param('id') id: string) {}

  @Post()
  create(@Body() dto: CreateUserDto) {}
}
```

becomes:

```txt
bruno/
├── bruno.json
└── Users/
    ├── find-all.bru      # GET  {{baseUrl}}/users
    ├── find-one.bru      # GET  {{baseUrl}}/users/:id     params:path { id: }
    └── create.bru        # POST {{baseUrl}}/users         body:json { {} }
```

Rename the handler or move the file — the request follows, in place. Change a
path — the request updates. Delete a route — its untouched request is deleted;
one you edited becomes a conflict that only you can resolve.

## Day-to-day workflow

```sh
nestbruno watch    # keep the collection in sync while you edit (Ctrl-C to stop)
nestbruno sync --dry-run   # see what a sync would do
nestbruno sync             # apply it
nestbruno routes           # inspect what was found (diagnostic output)
nestbruno discover         # list candidate files (diagnostic output)
```

Most users only ever need `nestbruno watch` during development and
`nestbruno sync` before committing.
