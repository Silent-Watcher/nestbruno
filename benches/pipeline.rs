//! Focused benchmarks for the four expensive stages of the pipeline:
//! TypeScript extraction, Bruno generation, discovery, and a no-op
//! synchronization (the most common `sync` case). The staged/watch paths are
//! not benched separately: they reuse these stages, and their own overhead is
//! the linear reconcile pass measured in `docs/phases/phase-06.md`.
//!
//! Run with `cargo bench`. Observations belong in `docs/phases/phase-08.md`.
//!
//! Scoped allowance (this file only): `unwrap_used`/`expect_used` — benchmark
//! setup may panic loudly; it is not user-facing code.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use nestbruno::bruno::generate;
use nestbruno::bruno::writer::write_collection;
use nestbruno::config::Config;
use nestbruno::discovery::{Matcher, discover_controllers};
use nestbruno::nest::extract::extract_controllers;
use nestbruno::project::Project;
use nestbruno::route::BodyShape;
use nestbruno::sync::{Desired, synchronize};

/// A controller with three routes (one with path param, one with body).
fn controller_source(i: usize) -> String {
    format!(
        "import {{ Controller, Get, Post, Param, Body }} from '@nestjs/common';\n\
         \n\
         @Controller('users{i}')\n\
         export class Users{i}Controller {{\n\
         \x20 @Get()\n\
         \x20 findAll() {{}}\n\
         \n\
         \x20 @Get(':id')\n\
         \x20 findOne(@Param('id') id: string) {{}}\n\
         \n\
         \x20 @Post()\n\
         \x20 create(@Body() dto: CreateUserDto{i}) {{}}\n\
         }}\n"
    )
}

fn synthetic_project(count: usize) -> String {
    (0..count)
        .map(controller_source)
        .collect::<Vec<_>>()
        .join("\n")
}

fn bench_extract(criterion: &mut Criterion) {
    // 100 controllers, 300 routes, one file — parse + semantic walk.
    let source = synthetic_project(100);
    let mut group = criterion.benchmark_group("extract");
    group.throughput(criterion::Throughput::Bytes(source.len() as u64));
    group.bench_function("100_controllers", |bench| {
        bench.iter(|| {
            let analysis = extract_controllers(
                Path::new("bench/bench.controller.ts"),
                black_box(&source),
                None,
                None,
                None,
                None,
            );
            assert_eq!(analysis.controllers.len(), 100);
        })
    });
    group.finish();
}

fn bench_generate(criterion: &mut Criterion) {
    // Pure planner from the route IR to desired requests.
    let source = synthetic_project(100);
    let analysis = extract_controllers(
        Path::new("bench/bench.controller.ts"),
        &source,
        None,
        None,
        None,
        None,
    );
    let config = Config::default();
    let mut group = criterion.benchmark_group("generate");
    group.bench_function("100_controllers_300_routes", |bench| {
        bench.iter(|| {
            let generation = generate(black_box(&config), black_box(&analysis.controllers));
            assert_eq!(generation.requests.len(), 300);
        })
    });
    group.finish();
}

/// A controller whose `@Post` body type is a class DTO defined in a separate
/// file, reached through a relative import — the Phase 11 cross-file case.
fn bench_extract_cross_file_dto(criterion: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("dto")).unwrap();
    let dto_source = "export class CreateUserDto {\n\
                      \x20 name: string;\n\
                      \x20 email: string;\n\
                      \x20 password: string;\n\
                      }\n";
    fs::write(src.join("dto/create-user.dto.ts"), dto_source).unwrap();
    let controller_source = "import { Controller, Get, Post, Body } from '@nestjs/common';\n\
                             import { CreateUserDto } from './dto/create-user.dto';\n\
                             \n\
                             @Controller('users')\n\
                             export class UsersController {\n\
                             \x20 @Get()\n\
                             \x20 findAll() {}\n\
                             \n\
                             \x20 @Post()\n\
                             \x20 create(@Body() dto: CreateUserDto) {}\n\
                             }\n";
    fs::write(src.join("users.controller.ts"), controller_source).unwrap();
    let project = Project::open(dir.path()).unwrap();
    let config = Config::default();
    let matcher = Matcher::new(&config).unwrap();

    let mut group = criterion.benchmark_group("extract_cross_file");
    group.bench_function("1_controller_1_shared_dto", |bench| {
        bench.iter(|| {
            let analysis = extract_controllers(
                Path::new("src/users.controller.ts"),
                black_box(controller_source),
                Some(project.root()),
                Some(&config.source),
                Some(&config),
                Some(&matcher),
            );
            assert_eq!(analysis.controllers.len(), 1);
            // The imported DTO must have been resolved to its three fields;
            // otherwise the benchmark would measure the Unknown fallback.
            assert_eq!(
                analysis.controllers[0].routes[1].body,
                BodyShape::Fields(vec![
                    "name".to_owned(),
                    "email".to_owned(),
                    "password".to_owned(),
                ])
            );
        })
    });
    group.finish();
}

/// A temp project with `count` TypeScript files, a third of them controllers.
fn discovery_tree(count: usize) -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..count {
        let name = if i % 3 == 0 {
            format!("mod{i}/users{i}.controller.ts")
        } else if i % 3 == 1 {
            format!("mod{i}/service{i}.ts")
        } else {
            format!("mod{i}/deep/nested{i}.ts")
        };
        let path = dir.path().join("src").join(&name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = if i % 3 == 0 {
            controller_source(i)
        } else {
            format!("export const value{i} = {i};\n")
        };
        fs::write(path, content).unwrap();
    }
    // node_modules must never be entered.
    let noise = dir.path().join("node_modules/pkg/index.controller.ts");
    fs::create_dir_all(noise.parent().unwrap()).unwrap();
    fs::write(noise, controller_source(9_999)).unwrap();
    let project = Project::open(dir.path()).unwrap();
    (dir, project)
}

fn bench_discover(criterion: &mut Criterion) {
    let (_dir, project) = discovery_tree(1_000);
    let mut group = criterion.benchmark_group("discover");
    group.bench_function("1000_files_334_controllers", |bench| {
        bench.iter(|| {
            let discovery = discover_controllers(black_box(&project)).unwrap();
            assert_eq!(discovery.files.len(), 334);
        })
    });
    group.finish();
}

/// Write a collection once, then measure a no-op synchronization (the common
/// case: nothing changed).
fn bench_sync_noop(criterion: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let source = synthetic_project(250);
    let analysis = extract_controllers(
        Path::new("bench/bench.controller.ts"),
        &source,
        None,
        None,
        None,
        None,
    );
    let config = Config::default();
    let generation = generate(&config, &analysis.controllers);
    assert_eq!(generation.requests.len(), 750);
    let report = write_collection(dir.path(), &config.bruno, &generation.requests, false).unwrap();
    assert_eq!(
        report.count(nestbruno::bruno::writer::FileStatus::Created),
        750
    );
    let desired = Desired {
        requests: &generation.requests,
        skipped: &[],
        complete: true,
    };

    let mut group = criterion.benchmark_group("sync");
    group.bench_function("no_op_750_requests", |bench| {
        bench.iter(|| {
            let result = synchronize(
                black_box(dir.path()),
                &config.bruno,
                black_box(&desired),
                false,
            )
            .unwrap();
            assert!(result.is_complete());
            assert!(result.unchanged.len() == 750);
        })
    });
    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(20).warm_up_time(std::time::Duration::from_millis(500)).measurement_time(std::time::Duration::from_secs(3));
    targets = bench_extract, bench_extract_cross_file_dto, bench_generate, bench_discover, bench_sync_noop
);
criterion_main!(benches);
