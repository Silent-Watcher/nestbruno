# NestBruno

NestBruno is a Rust command-line tool that keeps a [Bruno](https://www.usebruno.com/)
API collection in sync with the HTTP routes of a NestJS project, statically,
without running your application.

## Key features

- Static analysis only: your TypeScript is never executed.
- Cross-file DTO resolution: follows local imports to class, interface and
  Zod DTO definitions and generates JSON body skeletons (values `null`,
  max 64 fields).
- Safe synchronization: ownership stamps, conflict reporting, atomic
  writes; your hand-edited requests are never overwritten.
- Deterministic output, suitable for Git diffs.
- Watch mode and Git staged sync.

## Installation

```sh
cargo install nestbruno
```

Or from source: `cargo install --path .` (see
[github.com/Silent-Watcher/nestbruno](https://github.com/Silent-Watcher/nestbruno)).

## Basic usage

```sh
nestbruno init     # write a nestbruno.toml with the defaults
nestbruno check    # validate the configuration and project layout
nestbruno discover # list the files that may contain controllers
nestbruno routes   # print the HTTP routes found in them
nestbruno generate # write a Bruno request per route (never overwrites without --force)
nestbruno sync     # update the collection safely; your edits are never overwritten
nestbruno sync --dry-run   # show what sync would do
nestbruno watch    # keep the collection in sync while you edit
```

## Supported capabilities

See [docs/project-status.md](docs/project-status.md) for what is implemented
today and [docs/roadmap.md](docs/roadmap.md) for what is planned.
