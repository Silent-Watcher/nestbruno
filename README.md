# NestBruno

NestBruno is a Rust command-line tool that keeps a [Bruno](https://www.usebruno.com/)
API collection in sync with the HTTP routes of a NestJS project, statically,
without running your application.

## Key features

- Static analysis only: your TypeScript is never executed.
- Safe by design: configured paths cannot escape the project.
- Deterministic output, suitable for Git diffs.

## Installation

```sh
cargo install --path .
```

## Basic usage

```sh
nestbruno init     # write a nestbruno.toml with the defaults
nestbruno check    # validate the configuration and project layout
nestbruno discover # list the files that may contain controllers
nestbruno routes   # print the HTTP routes found in them
nestbruno generate # write a Bruno request per route (never overwrites without --force)
```

## Supported capabilities

See [docs/project-status.md](docs/project-status.md) for what is implemented
today and [docs/roadmap.md](docs/roadmap.md) for what is planned.
