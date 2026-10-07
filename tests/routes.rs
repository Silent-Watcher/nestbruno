//! Integration tests for route extraction: real projects, the real binary.

mod common;

use std::path::PathBuf;

use common::{TestProject, stderr, stdout};
use nestbruno::diag::Severity;
use nestbruno::discovery::ControllerFile;
use nestbruno::nest::analyze_files;
use nestbruno::nest::extract::MAX_SOURCE_BYTES;
use nestbruno::project::Project;

const USERS: &str = "\
import { Body, Controller, Delete, Get, Param, Patch, Post, Query } from '@nestjs/common';

@Controller('users')
export class UsersController {
  constructor(private readonly users: UsersService) {}

  @Get()
  findAll(@Query('page') page: number, @Query() filter: FilterDto) {}

  @Get(':id')
  findOne(@Param('id') id: string, @Query('include') include: string) {}

  @Post()
  create(@Body() dto: CreateUserDto) {}

  @Patch(':id')
  update(@Param('id') id: string, @Body() dto: UpdateUserDto) {}

  @Delete(':id')
  remove(@Param('id') id: string) {}
}
";

const AUTH: &str = "\
@Controller('auth')
export class AuthController {
  @Post('login')
  login(@Body('username') u: string, @Body('password') p: string, @Headers('x-trace') t: string) {}

  @Get('profile')
  profile() {}
}
";

fn reference_project() -> TestProject {
    let p = TestProject::new();
    p.write("src/users/users.controller.ts", USERS);
    p.write("src/auth/auth.controller.ts", AUTH);
    p.write("src/users/users.service.ts", "export class UsersService {}");
    p.write(
        "node_modules/x/x.controller.ts",
        "@Controller('evil') class E { @Get() h() {} }",
    );
    p
}

#[test]
fn routes_command_prints_the_extracted_routes() {
    let p = reference_project();
    let output = p.run(&["routes"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let expected = "\
POST /auth/login  AuthController.login  src/auth/auth.controller.ts:3:3
  @Body(username) #0
  @Body(password) #1
  @Headers(x-trace) #2
GET /auth/profile  AuthController.profile  src/auth/auth.controller.ts:6:3
GET /users  UsersController.findAll  src/users/users.controller.ts:7:3
  @Query(page) #0
  @Query() #1
GET /users/:id  UsersController.findOne  src/users/users.controller.ts:10:3
  @Param(id) #0
  @Query(include) #1
POST /users  UsersController.create  src/users/users.controller.ts:13:3
  @Body() #0
PATCH /users/:id  UsersController.update  src/users/users.controller.ts:16:3
  @Param(id) #0
  @Body() #1
DELETE /users/:id  UsersController.remove  src/users/users.controller.ts:19:3
  @Param(id) #0
";
    // Normalize separators so the comparison is platform independent.
    assert_eq!(stdout(&output).replace('\\', "/"), expected);
    assert_eq!(stderr(&output), "");
}

#[test]
fn output_is_deterministic_across_runs() {
    let p = reference_project();
    let first = p.run(&["routes"]);
    let second = p.run(&["routes"]);
    assert_eq!(stdout(&first), stdout(&second));
}

#[test]
fn controllers_in_nested_directories_and_several_files_are_all_extracted() {
    let p = TestProject::new();
    p.write(
        "src/modules/admin/users/users.controller.ts",
        "@Controller('admin/users') export class A { @Get() a() {} }\n@Controller('admin/roles') export class B { @Get() b() {} }",
    );
    p.write(
        "src/modules/public/ping.controller.ts",
        "@Controller() export class Ping { @Get('ping') ping() {} }",
    );
    let output = p.run(&["routes"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output).replace('\\', "/");
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    // Discovery order is sorted by path: admin/users, then public/ping.
    assert!(
        lines[0]
            .starts_with("GET /admin/users  A.a  src/modules/admin/users/users.controller.ts:1:")
    );
    assert!(
        lines[1]
            .starts_with("GET /admin/roles  B.b  src/modules/admin/users/users.controller.ts:2:")
    );
    assert!(lines[2].starts_with("GET /ping  Ping.ping  src/modules/public/ping.controller.ts:1:"));
}

#[test]
fn candidate_files_without_controllers_produce_nothing() {
    let p = TestProject::new();
    p.write(
        "src/a.controller.ts",
        "export class NotAController { @Get() h() {} }",
    );
    let output = p.run(&["routes"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "");
    let verbose = p.run(&["routes", "-v"]);
    assert!(
        stderr(&verbose).contains("no @Controller class found"),
        "{}",
        stderr(&verbose)
    );
}

#[test]
fn controllers_in_unconventionally_named_files_are_found_via_include() {
    let p = TestProject::new();
    p.write("nestbruno.toml", "include = [\"**/*-api.ts\"]\n");
    p.write(
        "src/custom-user-api.ts",
        "@Controller('users') class X { @Get() h() {} }",
    );
    let output = p.run(&["routes"]);
    assert!(stdout(&output).starts_with("GET /users  X.h  src/custom-user-api.ts:1:"));
}

#[test]
fn a_malformed_file_is_reported_and_valid_files_are_still_processed() {
    let p = TestProject::new();
    p.write(
        "src/a/good.controller.ts",
        "@Controller('good') class G { @Get() h() {} }",
    );
    p.write(
        "src/b/broken.controller.ts",
        "@Controller('bad')\nexport class B {\n  @Get(\n  h() {}\n",
    );
    p.write(
        "src/c/also.controller.ts",
        "@Controller('also') class A { @Post() h() {} }",
    );
    let output = p.run(&["routes"]);

    assert_eq!(
        output.status.code(),
        Some(4),
        "incomplete results use exit code 4"
    );
    let out = stdout(&output).replace('\\', "/");
    assert!(out.contains("GET /good  G.h"), "{out}");
    assert!(out.contains("POST /also  A.h"), "{out}");
    assert!(!out.contains("/bad"), "{out}");

    let err = stderr(&output).replace('\\', "/");
    assert!(err.contains("error: src/b/broken.controller.ts:"), "{err}");
    assert!(err.contains("unable to parse"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn unresolved_paths_are_reported_as_warnings_not_failures() {
    let p = TestProject::new();
    p.write(
        "src/a.controller.ts",
        "const prefix = 'x';\n@Controller(prefix)\nclass A {\n  @Get(':id') h(@Param(KEY) id: string) {}\n}\n@Controller('ok') class B { @Get(build()) h() {} }",
    );
    let output = p.run(&["routes"]);
    assert!(
        output.status.success(),
        "warnings do not change the exit code"
    );
    let out = stdout(&output);
    assert!(
        out.contains("GET <unresolved>  A.h  src/a.controller.ts:4:3"),
        "{out}"
    );
    assert!(out.contains("@Param(<unresolved>) #0"), "{out}");
    let err = stderr(&output);
    assert!(err.contains("warning: src/a.controller.ts:2:1"), "{err}");
    assert!(err.contains("cannot be determined statically"), "{err}");
}

#[test]
fn invalid_utf8_files_are_reported_per_file() {
    let p = TestProject::new();
    p.write(
        "src/ok.controller.ts",
        "@Controller('ok') class O { @Get() h() {} }",
    );
    std::fs::write(
        p.root().join("src/bin.controller.ts"),
        b"@Controller('\xff\xfe') class B {}",
    )
    .unwrap();
    let output = p.run(&["routes"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(stdout(&output).contains("GET /ok"));
    assert!(stderr(&output).contains("not valid UTF-8"));
}

#[test]
fn oversized_files_are_rejected_without_being_parsed() {
    let p = TestProject::new();
    let big = format!(
        "// {}\n@Controller('big') class C {{ @Get() h() {{}} }}",
        "x".repeat(MAX_SOURCE_BYTES)
    );
    p.write("src/big.controller.ts", &big);
    p.write(
        "src/ok.controller.ts",
        "@Controller('ok') class O { @Get() h() {} }",
    );
    let output = p.run(&["routes"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(stderr(&output).contains("too large"), "{}", stderr(&output));
    assert!(stdout(&output).contains("/ok"));
    assert!(!stdout(&output).contains("/big"));
}

#[test]
fn routes_fails_cleanly_when_discovery_is_fatal() {
    let p = TestProject::new();
    let output = p.run(&["routes"]);
    assert_eq!(output.status.code(), Some(3));
}

// ------------------------------------------------- adversarial nesting

/// Source texts that drive the parser to its deepest recursion, each exactly
/// at the size limit. These were the constructs with the highest measured
/// stack use per input byte.
fn worst_case_sources() -> Vec<(&'static str, String)> {
    let fill = |open: &str, middle: &str, close: &str| {
        let levels = (MAX_SOURCE_BYTES - middle.len()) / (open.len() + close.len());
        format!("{}{middle}{}", open.repeat(levels), close.repeat(levels))
    };
    vec![
        ("parentheses", fill("(", "1", ")")),
        ("decorator calls", fill("@b(", "1", ")")),
        (
            "unary operators",
            format!("const x = {}1;", "!".repeat(MAX_SOURCE_BYTES - 20)),
        ),
        (
            "arrow chain",
            format!("const x = {}1;", "a=>".repeat((MAX_SOURCE_BYTES - 20) / 3)),
        ),
        (
            "function types",
            format!(
                "type T = {}number;",
                "()=>".repeat((MAX_SOURCE_BYTES - 20) / 4)
            ),
        ),
        ("brackets", fill("[", "", "]")),
        ("blocks", fill("{", "", "}")),
        (
            "new chain",
            format!("const x = {}a;", "new ".repeat((MAX_SOURCE_BYTES - 20) / 4)),
        ),
        (
            "ternary chain",
            format!("const x = {}1;", "a?b:".repeat((MAX_SOURCE_BYTES - 20) / 4)),
        ),
        ("generic types", fill("A<", "number", ">")),
    ]
}

#[test]
fn worst_case_nesting_at_the_size_limit_does_not_crash_the_analysis() {
    let p = TestProject::new();
    let mut files = Vec::new();
    for (index, (name, source)) in worst_case_sources().into_iter().enumerate() {
        assert!(source.len() <= MAX_SOURCE_BYTES, "{name}: {}", source.len());
        let path = format!("src/n{index}.controller.ts");
        p.write(&path, &source);
        files.push(ControllerFile {
            path: PathBuf::from(path),
        });
    }
    let project = Project::open(p.root()).unwrap();

    // Surviving this call at all is the assertion: a stack overflow would
    // abort the whole test process. Each file must also be reported.
    let analysis = analyze_files(&project, &files).unwrap();
    assert!(analysis.controllers.is_empty());
    assert_eq!(
        analysis.diagnostics.len(),
        files.len(),
        "{:?}",
        analysis.diagnostics
    );
}

#[test]
fn deeply_nested_input_through_the_cli_does_not_abort_the_process() {
    let p = TestProject::new();
    p.write(
        "src/deep.controller.ts",
        &format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000)),
    );
    p.write(
        "src/ok.controller.ts",
        "@Controller('ok') class O { @Get() h() {} }",
    );
    let output = p.run(&["routes"]);
    // Whether the parser accepts or rejects the file, the process must exit
    // normally and the other file must be processed.
    assert!(
        output.status.code().is_some(),
        "terminated by a signal: {:?}",
        output.status
    );
    assert!(stdout(&output).contains("GET /ok"));
}

// ------------------------------------------------------- file hazards

#[cfg(unix)]
#[test]
fn files_replaced_by_symlinks_or_directories_after_discovery_are_not_read() {
    use std::os::unix::fs::symlink;

    let p = TestProject::new();
    p.write(
        "secret.txt",
        "@Controller('leak') class L { @Get() h() {} }",
    );
    p.write(
        "src/ok.controller.ts",
        "@Controller('ok') class O { @Get() h() {} }",
    );
    symlink(
        p.root().join("secret.txt"),
        p.root().join("src/link.controller.ts"),
    )
    .unwrap();
    std::fs::create_dir(p.root().join("src/dir.controller.ts")).unwrap();

    let project = Project::open(p.root()).unwrap();
    let files: Vec<_> = [
        "src/dir.controller.ts",
        "src/link.controller.ts",
        "src/ok.controller.ts",
        "src/gone.controller.ts",
    ]
    .iter()
    .map(|path| ControllerFile {
        path: PathBuf::from(path),
    })
    .collect();
    let analysis = analyze_files(&project, &files).unwrap();

    let names: Vec<_> = analysis
        .controllers
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["O"]);
    let errors = analysis
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    assert_eq!(errors, 3, "{:?}", analysis.diagnostics);
    assert!(analysis.is_incomplete());
}

#[test]
fn unreadable_file_is_reported_without_stopping_the_run() {
    let p = TestProject::new();
    let project = Project::open(p.root()).unwrap();
    let files = [ControllerFile {
        path: PathBuf::from("src/missing.controller.ts"),
    }];
    let analysis = analyze_files(&project, &files).unwrap();
    assert!(analysis.is_incomplete());
    assert!(analysis.diagnostics[0].message.contains("cannot read file"));
}
