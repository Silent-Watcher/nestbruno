# Contributing

## Workflow

1. Branch from `main`: `feat/<topic>`, `fix/<topic>`, `docs/<topic>`.
2. Make small, atomic commits.
3. Before every commit run `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo test --all-features`, then review `git diff` and `git status`.
4. Open a pull request. CI must be green.

## Commit conventions

`type: summary` with types `feat`, `fix`, `test`, `docs`, `chore`, `refactor`,
`perf`. Never commit code that fails formatting, lint or tests.

## Tests required

Every feature ships with tests. Every bug fix starts with a failing regression
test that is kept permanently. Tests verify behavior, are deterministic, run
offline, and do not depend on the working directory, system time, or the
user's environment.

## Documentation

Behavior changes update `docs/` in the same pull request. `README.md` stays
short (what it is, features, installation, basic usage, capabilities); long
documentation belongs in `docs/`.

## Dependencies

Before adding a dependency, state in the PR what problem it solves, why the
standard library or an existing dependency is not enough, and its maintenance
and security profile.
