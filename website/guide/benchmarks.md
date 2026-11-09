# Benchmarks

NestBruno ships a Criterion benchmark suite (`benches/pipeline.rs`) that
measures the four expensive pipeline stages plus cross-file DTO
resolution. This page records the current numbers and how to reproduce
them. Every claim on this site traces back to these benchmarks or to the
test suite.

## Results

Measured **2026-11-09**, release profile, Criterion median estimates:

| Benchmark | Median | Workload |
| --- | ---: | --- |
| `extract_cross_file/1_controller_1_shared_dto` | **40 µs** | One controller (3 routes); its `@Post` body type resolved from a DTO class in a separate file through a relative import |
| `extract/100_controllers` | **540 µs** | 100 controllers / 300 routes parsed and extracted from a single ~24 KiB file |
| `generate/100_controllers_300_routes` | **2.5 ms** | Pure planner: route IR → 300 desired `.bru` requests |
| `discover/1000_files_334_controllers` | **11.8 ms** | Walk of a 1,000-file tree (334 controllers, `node_modules` noise skipped) |
| `sync/no_op_750_requests` | **13.4 ms** | The common case: 750-request collection, nothing changed |

## Environment

- CPU: 12th Gen Intel Core i7-1255U (12 threads)
- OS: Linux 5.15.0-160 (Ubuntu-based sandbox)
- Rust: rustc 1.99.0, release profile
- NestBruno: v0.2.0, repository commit `3c0dbf5` plus the cross-file
  benchmark addition
- Harness: Criterion 0.5 — `sample_size(20)`, 500 ms warm-up, 3 s
  measurement per benchmark, `black_box` on all inputs, and an in-iteration
  assertion so a benchmark cannot be optimized into measuring nothing

## How to read these numbers

- **These are component benchmarks, not full-pipeline timings.** Each
  measures one stage in isolation. A real `sync` run adds filesystem
  traversal, collection reading and writing, which dominate wall-clock
  time on cold caches.
- **Workloads are synthetic and small.** They are sized to keep each
  measurement statistically stable, not to represent your repository.
- **Numbers are not portable.** Absolute times vary with hardware, cache
  state and build flags (up to ~2× across machines was observed for the
  watch pipeline). Treat the relative relationships — extraction is
  microseconds-scale per file, generation and sync are linear in request
  count — as the durable claims, not the exact values.
- The extraction benchmarks parse the same source repeatedly; a real run
  reads each file once from disk. The cross-file benchmark includes the
  dependency file read + parse on every iteration, matching the current
  per-route resolution behavior.

## Reproducing

```sh
cargo bench
```

Criterion writes detailed reports (distributions, change detection against
the previous run) to `target/criterion/`. Open
`target/criterion/report/index.html` for the full HTML report.

## What is not (yet) benchmarked

- Watch-mode invalidation latency and Staged Sync over large Git indexes —
  both have measured observations in the repository phase documents
  (`docs/phases/phase-06.md`, phase-07), but no Criterion harness yet.
- Peak resident memory. The design keeps bounded per-file buffers and
  arena-allocated ASTs that are dropped per file; no reproducible
  memory benchmark ships with the suite.
- Bulk cross-file resolution (many controllers sharing DTO files). The
  current resolver builds its per-run cache per route, so shared
  dependencies are re-read across routes — a known limitation recorded in
  the repository's known-issues list.
