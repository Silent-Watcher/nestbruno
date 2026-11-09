//! Integration tests for `nestbruno explain` (Phase 15): the real binary
//! explains what the existing analyzer found — never re-analyzing, never
//! writing.
//!
//! Scoped allowance (this file only): `clippy::expect_used` — broken test
//! setup should panic with a message, and clippy's test exemption does not
//! cover integration-test crates.
#![allow(clippy::expect_used)]

mod common;

use common::{TestProject, stderr, stdout};

const USERS: &str = "\
import { Body, Controller, Get, Param, Post, Query } from '@nestjs/common';

class CreateUserDto {
  name: string;
}

@Controller('users')
export class UsersController {
  @Get()
  findAll(@Query('page') page: number) {}

  @Get(':id')
  findOne(@Param('id') id: string, @Query() filter: FilterDto) {}

  @Post()
  create(@Body() dto: CreateUserDto) {}
}
";

const AUTH: &str = "\
import { Controller, Post } from '@nestjs/common';

@Controller('auth')
export class AuthController {
  @Post('login')
  login() {}
}
";

fn project() -> TestProject {
    let p = TestProject::new();
    p.write("src/users/users.controller.ts", USERS);
    p
}

/// Parse the whole stdout as one JSON document.
fn json(stdout: &str) -> serde_json::Value {
    match serde_json::from_str(stdout) {
        Ok(value) => value,
        Err(err) => panic!("stdout is not a single JSON document: {err}\n{stdout}"),
    }
}

#[test]
fn explain_reports_identification_resolution_and_metadata() {
    let p = project();
    let output = p.run(&["explain", "GET /users/:id"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);

    assert!(text.contains("Route Explanation"), "{text}");
    assert!(text.contains("Route:      GET /users/:id"), "{text}");
    assert!(text.contains("Controller: UsersController"), "{text}");
    assert!(text.contains("Handler:    findOne"), "{text}");
    assert!(
        text.contains("Source:     src/users/users.controller.ts:12:3"),
        "{text}"
    );

    assert!(text.contains("Resolution:"), "{text}");
    assert!(text.contains("Controller prefix: /users"), "{text}");
    assert!(text.contains("Handler path:      /:id"), "{text}");
    assert!(text.contains("Final route:       /users/:id"), "{text}");
    assert!(text.contains("Global prefix:     none"), "{text}");

    assert!(text.contains("Metadata:"), "{text}");
    assert!(text.contains("- @Param(id) #0 (string)"), "{text}");
    assert!(text.contains("- @Query() #1"), "{text}");
    assert!(text.contains("Body: none"), "{text}");

    assert!(text.contains("Analysis:"), "{text}");
    assert!(text.contains("Controller identified"), "{text}");
    assert!(text.contains("HTTP method resolved"), "{text}");
    assert!(
        text.contains("Warnings:") && text.contains("None"),
        "{text}"
    );
}

#[test]
fn explain_reports_method_decorator_and_parameter_kinds() {
    let p = project();
    let output = p.run(&["explain", "POST /users"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    // The DTO is declared in the same file: its fields are extracted.
    assert!(text.contains("Body: fields: name"), "{text}");
    assert!(text.contains("- @Body() #0"), "{text}");
}

#[test]
fn explain_reports_the_resolved_dto_fields_of_a_query_object() {
    let p = TestProject::new();
    p.write(
        "src/users/users.controller.ts",
        "\
import { Controller, Get, Query } from '@nestjs/common';

class FilterDto {
  page: number;
  limit: number;
}

@Controller('users')
export class UsersController {
  @Get()
  findAll(@Query() filter: FilterDto) {}
}
",
    );
    let output = p.run(&["explain", "--handler", "findAll"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("[fields: page, limit]"), "{text}");
}

#[test]
fn explain_selects_by_handler_and_controller() {
    let p = project();
    p.write("src/auth/auth.controller.ts", AUTH);
    let output = p.run(&[
        "explain",
        "--controller",
        "AuthController",
        "--handler",
        "login",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Route:      POST /auth/login"), "{text}");
    assert!(text.contains("Controller: AuthController"), "{text}");
}

#[test]
fn explain_resolves_the_global_prefix_and_versioning_from_the_bootstrap() {
    let p = project();
    p.write(
        "src/main.ts",
        "\
import { VersioningType } from '@nestjs/common';
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api');
  app.enableVersioning({ type: VersioningType.URI, defaultVersion: '1' });
}
bootstrap();
",
    );
    let output = p.run(&["explain", "GET /api/v1/users/:id"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Global prefix:     api (applied)"), "{text}");
    assert!(text.contains("Version segment:   v1"), "{text}");
    assert!(text.contains("Handler path:      /:id"), "{text}");
    assert!(
        text.contains("Final route:       /api/v1/users/:id"),
        "{text}"
    );
    assert!(text.contains("global prefix api + version v1 + controller path /users + handler path /:id → /api/v1/users/:id"), "{text}");
}

#[test]
fn explain_reports_dynamic_paths_as_unresolved() {
    let p = TestProject::new();
    p.write(
        "src/users/users.controller.ts",
        "\
const base = 'users';

@Controller(base)
export class UsersController {
  @Get()
  findAll() {}
}
",
    );
    let output = p.run(&["explain", "--handler", "findAll"]);
    let text = stdout(&output);
    if output.status.code() == Some(1) {
        // If the route was not extracted at all, the honest answer is
        // "no route matches" — also acceptable, never a fabricated one.
        assert!(
            stderr(&output).contains("no route matches"),
            "{}",
            stderr(&output)
        );
        return;
    }
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(text.contains("Final route:       <unresolved>"), "{text}");
    assert!(text.contains("Note:"), "{text}");
}

#[test]
fn a_missing_route_fails_with_a_clear_message() {
    let p = project();
    let output = p.run(&["explain", "DELETE /users"]);
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.contains("no route matches DELETE /users"),
        "{message}"
    );
    assert!(stdout(&output).is_empty());
}

#[test]
fn an_ambiguous_selector_lists_all_candidates() {
    // Two controllers in different files, same handler name: the handler
    // selector alone is ambiguous and every candidate is listed.
    let p = TestProject::new();
    p.write("src/a/a.controller.ts", &USERS.replace("'users'", "'a'"));
    p.write("src/b/b.controller.ts", &USERS.replace("'users'", "'b'"));
    let output = p.run(&["explain", "--handler", "findAll"]);
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(message.contains("2 routes match"), "{message}");
    assert!(message.contains("UsersController.findAll"), "{message}");
    assert!(stdout(&output).is_empty());
}

#[test]
fn the_file_flag_disambiguates_and_restricts() {
    let p = project();
    p.write("src/auth/auth.controller.ts", AUTH);
    let same = USERS.replace("'users'", "'users2'");
    p.write("src/users2/users.controller.ts", &same);
    // Both controllers have findAll with GET /users2? Only users2's path is
    // /users2; the shared handler name is disambiguated by --file.
    let output = p.run(&[
        "explain",
        "--handler",
        "findAll",
        "--file",
        "src/auth/auth.controller.ts",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no route matches"),
        "{}",
        stderr(&output)
    );

    let output = p.run(&[
        "explain",
        "--handler",
        "findAll",
        "--file",
        "src/users/users.controller.ts",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Controller: UsersController"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn an_invalid_selector_is_a_usage_error() {
    let p = project();
    for selector in ["users", "GET", "FETCH /users", "GET /users extra"] {
        let output = p.run(&["explain", selector]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{selector}: {}",
            stderr(&output)
        );
    }
    let output = p.run(&["explain", "--controller", "UsersController"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "--controller without --handler must be rejected"
    );
    let output = p.run(&["explain"]);
    assert_eq!(output.status.code(), Some(2), "no selector at all");
}

#[test]
fn explain_json_is_a_single_stable_document() {
    let p = project();
    let output = p.run(&["explain", "GET /users/:id", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let doc = json(&stdout(&output));
    assert_eq!(doc["found"], true);
    assert_eq!(doc["complete"], true);
    assert_eq!(doc["route"]["method"], "GET");
    assert_eq!(doc["route"]["path"], "/users/:id");
    assert_eq!(doc["route"]["controller"], "UsersController");
    assert_eq!(doc["route"]["source"]["line"], 12);
    assert_eq!(
        doc["resolution"],
        serde_json::json!({
            "controller_prefix": "/users",
            "handler_path": "/:id",
            "final_path": "/users/:id",
            "global_prefix": null,
            "prefix_applied": false,
            "version_segment": null
        })
    );
    assert_eq!(doc["parameters"][0]["kind"], "param");
    assert_eq!(doc["body"], serde_json::json!({"shape": "none"}));
    let findings: Vec<&str> = doc["trace"]
        .as_array()
        .expect("trace")
        .iter()
        .map(|step| step["finding"].as_str().expect("finding"))
        .collect();
    assert!(findings.contains(&"Route path composed"));
    assert_eq!(doc["diagnostics"], serde_json::json!([]));

    let again = stdout(&p.run(&["explain", "GET /users/:id", "--format", "json"]));
    assert_eq!(stdout(&output), again, "deterministic output");
}

#[test]
fn explain_json_on_an_ambiguous_selector_lists_matches() {
    let p = TestProject::new();
    p.write("src/a/a.controller.ts", &USERS.replace("'users'", "'a'"));
    p.write("src/b/b.controller.ts", &USERS.replace("'users'", "'b'"));
    // Two controllers with the same handler; select by handler only.
    let output = p.run(&["explain", "--handler", "findAll", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json(&stdout(&output));
    assert_eq!(doc["found"], false);
    let matches = doc["matches"].as_array().expect("matches");
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0]["path"], "/a");
    assert!(stderr(&output).contains("2 routes match"));
}

#[test]
fn explain_json_on_a_missing_route_is_a_valid_document() {
    let p = project();
    let output = p.run(&["explain", "DELETE /users", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json(&stdout(&output));
    assert_eq!(doc["found"], false);
    assert_eq!(doc["matches"], serde_json::json!([]));
    assert!(stderr(&output).contains("no route matches"));
}

#[test]
fn explain_is_read_only_and_separates_stdout_from_stderr() {
    let p = project();
    let output = p.run(&["explain", "GET /users/:id"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).starts_with("Route Explanation"));
    assert_eq!(
        stderr(&output),
        "",
        "no diagnostics expected for this project"
    );
}
