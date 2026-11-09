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
use nestbruno::nest::ProjectContext;
use nestbruno::nest::extract::extract_controllers;
use nestbruno::project::Project;
use nestbruno::resolve::ResolverContext;
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
            );
            assert_eq!(analysis.controllers.len(), 100);
        })
    });
    group.finish();
}

fn bench_generate(criterion: &mut Criterion) {
    // Pure planner from the route IR to desired requests.
    let source = synthetic_project(100);
    let analysis = extract_controllers(Path::new("bench/bench.controller.ts"), &source, None);
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

const SHARED_DTO_SOURCE: &str = "export class CreateUserDto {\n\
                                 \x20 name: string;\n\
                                 \x20 email: string;\n\
                                 \x20 password: string;\n\
                                 }\n";

/// A controller whose `@Post` body type is a class DTO defined in a separate
/// file, reached through a relative import — the Phase 11 cross-file case.
fn controller_with_dto_source(i: usize, routes: usize) -> String {
    let mut source = format!(
        "import {{ Controller, Get, Post, Body }} from '@nestjs/common';\n\
         import {{ CreateUserDto }} from './dto/create-user.dto';\n\
         \n\
         @Controller('users{i}')\n\
         export class Users{i}Controller {{\n\
         \x20 @Get()\n\
         \x20 findAll() {{}}\n"
    );
    for route in 0..routes {
        source.push_str(&format!(
            "\n\
             \x20 @Post('r{route}')\n\
             \x20 create{route}(@Body() dto: CreateUserDto) {{}}\n"
        ));
    }
    source.push_str("}\n");
    source
}

/// Analyze `sources` (controller file texts, at `src/mod{i}/users.controller.ts`)
/// through one run-scoped resolver context — the shape of a real analysis run.
fn analyze_one_run(
    root: &Path,
    config: &Config,
    matcher: &Matcher,
    sources: &[(String, String)],
) -> usize {
    let mut resolver = ResolverContext::new(root);
    let mut controllers = 0;
    for (relative, source) in sources {
        let mut project_context = ProjectContext {
            root,
            source_prefix: &config.source,
            config,
            matcher,
            resolver: &mut resolver,
        };
        let analysis = extract_controllers(Path::new(relative), source, Some(&mut project_context));
        controllers += analysis.controllers.len();
    }
    controllers
}

/// Assert that every analyzed controller resolved its DTO to the three
/// fields, so a benchmark can never silently degrade to the Unknown fallback.
fn assert_resolved(analysis: &nestbruno::nest::extract::FileAnalysis, routes: usize) {
    assert_eq!(analysis.controllers.len(), 1);
    for route in &analysis.controllers[0].routes[1..=routes] {
        assert_eq!(
            route.body,
            BodyShape::Fields(vec![
                "name".to_owned(),
                "email".to_owned(),
                "password".to_owned(),
            ])
        );
    }
}

fn bench_extract_cross_file_dto(criterion: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("dto")).unwrap();
    fs::write(src.join("dto/create-user.dto.ts"), SHARED_DTO_SOURCE).unwrap();
    let single_controller = controller_with_dto_source(0, 1);
    fs::write(src.join("users.controller.ts"), &single_controller).unwrap();
    let project = Project::open(dir.path()).unwrap();
    let config = Config::default();
    let matcher = Matcher::new(&config).unwrap();

    let mut group = criterion.benchmark_group("extract_cross_file");
    group.bench_function("1_controller_1_shared_dto", |bench| {
        bench.iter(|| {
            let mut project_context = ProjectContext {
                root: project.root(),
                source_prefix: &config.source,
                config: &config,
                matcher: &matcher,
                resolver: &mut ResolverContext::new(project.root()),
            };
            let analysis = extract_controllers(
                Path::new("src/users.controller.ts"),
                black_box(&single_controller),
                Some(&mut project_context),
            );
            assert_resolved(&analysis, 1);
        })
    });

    // Eight routes in one controller sharing the same DTO: the pre-11.1.2
    // behavior re-read and re-parsed the controller and the DTO per route.
    group.bench_function("1_controller_8_routes_shared_dto", |bench| {
        bench.iter(|| {
            let source = controller_with_dto_source(0, 8);
            let mut project_context = ProjectContext {
                root: project.root(),
                source_prefix: &config.source,
                config: &config,
                matcher: &matcher,
                resolver: &mut ResolverContext::new(project.root()),
            };
            let analysis = extract_controllers(
                Path::new("src/users.controller.ts"),
                black_box(&source),
                Some(&mut project_context),
            );
            assert_resolved(&analysis, 8);
        })
    });
    group.finish();

    // Eight controller files importing the same shared DTO, analyzed through
    // one run: measures the module cache across controllers.
    let shared_dir = tempfile::tempdir().unwrap();
    let shared_src = shared_dir.path().join("src");
    fs::create_dir_all(shared_src.join("dto")).unwrap();
    fs::write(shared_src.join("dto/create-user.dto.ts"), SHARED_DTO_SOURCE).unwrap();
    let shared_sources: Vec<(String, String)> = (0..8)
        .map(|i| {
            (
                format!("src/mod{i}/users{i}.controller.ts"),
                controller_with_dto_source(i, 1),
            )
        })
        .collect();
    for (relative, source) in &shared_sources {
        let path = shared_dir.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    let shared_project = Project::open(shared_dir.path()).unwrap();
    let mut group = criterion.benchmark_group("extract_cross_file");
    group.bench_function("8_controllers_1_shared_dto", |bench| {
        bench.iter(|| {
            let controllers = analyze_one_run(
                shared_project.root(),
                &config,
                &matcher,
                black_box(&shared_sources),
            );
            assert_eq!(controllers, 8);
        })
    });

    // The DTO reached through a four-reexport barrel chain (the hop limit).
    let chain_dir = tempfile::tempdir().unwrap();
    let chain_src = chain_dir.path().join("src");
    fs::create_dir_all(chain_src.join("dto")).unwrap();
    fs::write(chain_src.join("dto/create-user.dto.ts"), SHARED_DTO_SOURCE).unwrap();
    for i in 1..=4 {
        let from = if i == 1 {
            "./dto/create-user.dto".to_owned()
        } else {
            format!("./barrel{}", i - 1)
        };
        fs::write(
            chain_src.join(format!("barrel{i}.ts")),
            format!("export {{ {{ CreateUserDto }} }} from '{from}';\n"),
        )
        .unwrap();
    }
    let chain_controller = "import { Controller, Get, Post, Body } from '@nestjs/common';\n\
                            import { CreateUserDto } from './barrel4';\n\
                            \n\
                            @Controller('users')\n\
                            export class UsersController {\n\
                            \x20 @Get()\n\
                            \x20 findAll() {}\n\
                            \n\
                            \x20 @Post()\n\
                            \x20 create(@Body() dto: CreateUserDto) {}\n\
                            }\n";
    fs::write(chain_src.join("users.controller.ts"), chain_controller).unwrap();
    let chain_sources = vec![(
        "src/users.controller.ts".to_owned(),
        chain_controller.to_owned(),
    )];
    let chain_project = Project::open(chain_dir.path()).unwrap();
    group.bench_function("4_hops_reexport_chain", |bench| {
        bench.iter(|| {
            let controllers = analyze_one_run(
                chain_project.root(),
                &config,
                &matcher,
                black_box(&chain_sources),
            );
            assert_eq!(controllers, 1);
        })
    });

    // A larger graph within the configured limits: eight controllers, eight
    // distinct DTOs each (64 dependency files), all sharing one run context.
    let graph_dir = tempfile::tempdir().unwrap();
    let graph_src = graph_dir.path().join("src");
    fs::create_dir_all(graph_src.join("dto")).unwrap();
    let mut graph_sources = Vec::new();
    for i in 0..8 {
        let mut imports = String::new();
        let mut routes = String::new();
        for j in 0..8 {
            imports.push_str(&format!(
                "import {{ Dto{i}_{j} }} from './dto/dto{i}_{j}';\n"
            ));
            routes.push_str(&format!(
                "\n\
                 \x20 @Post('r{j}')\n\
                 \x20 create{j}(@Body() dto: Dto{i}_{j}) {{}}\n"
            ));
            fs::write(
                graph_src.join(format!("dto/dto{i}_{j}.ts")),
                format!("export class Dto{i}_{j} {{\n  field0: string;\n  field1: number;\n}}\n"),
            )
            .unwrap();
        }
        let source = format!(
            "import {{ Controller, Get, Post, Body }} from '@nestjs/common';\n\
             {imports}\n\
             @Controller('g{i}')\n\
             export class G{i}Controller {{\n\
             \x20 @Get()\n\
             \x20 findAll() {{}}\n\
             {routes}}}\n"
        );
        let relative = format!("src/mod{i}/graph{i}.controller.ts");
        let path = graph_dir.path().join(&relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &source).unwrap();
        graph_sources.push((relative, source));
    }
    let graph_project = Project::open(graph_dir.path()).unwrap();
    group.bench_function("8_controllers_8_dtos_each", |bench| {
        bench.iter(|| {
            let controllers = analyze_one_run(
                graph_project.root(),
                &config,
                &matcher,
                black_box(&graph_sources),
            );
            assert_eq!(controllers, 8);
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
    let analysis = extract_controllers(Path::new("bench/bench.controller.ts"), &source, None);
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
