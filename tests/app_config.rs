//! Acceptance tests for global prefix and URI versioning: real bootstrap
//! files, the real binary, the actual `.bru` output.

// Scoped allowance (this file only): the harness panics with a message when
// the project under test cannot be set up, the right failure mode for test
// infrastructure; clippy's test exemption does not cover integration files.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{TestProject, stderr};

const BOOTSTRAP_WITH_PREFIX: &str = "\
import { NestFactory } from '@nestjs/core';
import { AppModule } from './app.module';

async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api');
  await app.listen(3000);
}
bootstrap();
";

fn controller(path: &str) -> String {
    format!(
        "import {{ Controller, Get, Post }} from '@nestjs/common';\n\
         \n\
         @Controller('{path}')\n\
         export class UsersController {{\n\
         \x20 @Get()\n\
         \x20 findAll() {{}}\n\
         \n\
         \x20 @Post()\n\
         \x20 create() {{}}\n\
         }}\n"
    )
}

fn request_url(p: &TestProject, request: &str) -> String {
    let content = String::from_utf8_lossy(
        &std::fs::read(p.root().join("bruno").join(request)).expect("request file"),
    )
    .into_owned();
    content
        .lines()
        .find(|line| line.starts_with("  url: "))
        .expect("url line")
        .to_owned()
}

#[test]
fn a_static_global_prefix_is_applied_to_every_route() {
    let p = TestProject::new();
    p.write("src/main.ts", BOOTSTRAP_WITH_PREFIX);
    p.write("src/users/users.controller.ts", &controller("users"));
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        request_url(&p, "Users/find-all.bru"),
        "  url: {{baseUrl}}/api/users"
    );
    assert_eq!(
        request_url(&p, "Users/create.bru"),
        "  url: {{baseUrl}}/api/users"
    );
}

#[test]
fn exclude_rules_keep_matching_routes_unprefixed() {
    let p = TestProject::new();
    p.write(
        "src/main.ts",
        &BOOTSTRAP_WITH_PREFIX.replace(
            "  app.setGlobalPrefix('api');",
            "  app.setGlobalPrefix('api', { exclude: ['users'] });",
        ),
    );
    p.write("src/users/users.controller.ts", &controller("users"));
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        request_url(&p, "Users/find-all.bru"),
        "  url: {{baseUrl}}/users"
    );
}

#[test]
fn uri_versioning_composes_prefix_version_and_path() {
    let p = TestProject::new();
    p.write(
        "src/main.ts",
        &BOOTSTRAP_WITH_PREFIX.replace(
            "  app.setGlobalPrefix('api');",
            "  app.setGlobalPrefix('api');\n  app.enableVersioning({ type: VersioningType.URI, defaultVersion: '1' });",
        )
        .replace(
            "import { NestFactory } from '@nestjs/core';",
            "import { NestFactory } from '@nestjs/core';\nimport { VersioningType } from '@nestjs/common';",
        ),
    );
    p.write("src/users/users.controller.ts", &controller("users"));
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        request_url(&p, "Users/find-all.bru"),
        "  url: {{baseUrl}}/api/v1/users"
    );
}

#[test]
fn controller_and_method_versions_follow_nestjs_precedence() {
    let p = TestProject::new();
    p.write(
        "src/main.ts",
        &BOOTSTRAP_WITH_PREFIX.replace(
            "  app.setGlobalPrefix('api');",
            "  app.setGlobalPrefix('api');\n  app.enableVersioning({ type: VersioningType.URI, defaultVersion: '1' });",
        )
        .replace(
            "import { NestFactory } from '@nestjs/core';",
            "import { NestFactory } from '@nestjs/core';\nimport { Controller, Get, Post, Version, VERSION_NEUTRAL } from '@nestjs/common';",
        ),
    );
    // The controller pins version 2; one method overrides to 3; one is
    // version-neutral.
    p.write(
        "src/users/users.controller.ts",
        r#"
import { Controller, Get, Post, Version, VERSION_NEUTRAL } from '@nestjs/common';

@Controller({ path: 'users', version: '2' })
export class UsersController {
  @Get()
  findAll() {}

  @Version('3')
  @Post()
  create() {}

  @Version(VERSION_NEUTRAL)
  @Get('health')
  health() {}
}
"#,
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        request_url(&p, "Users/find-all.bru"),
        "  url: {{baseUrl}}/api/v2/users"
    );
    assert_eq!(
        request_url(&p, "Users/create.bru"),
        "  url: {{baseUrl}}/api/v3/users"
    );
    assert_eq!(
        request_url(&p, "Users/health.bru"),
        "  url: {{baseUrl}}/api/users/health"
    );
}

#[test]
fn multiple_declared_versions_become_multiple_requests() {
    let p = TestProject::new();
    p.write(
        "src/main.ts",
        &BOOTSTRAP_WITH_PREFIX.replace(
            "import { NestFactory } from '@nestjs/core';",
            "import { NestFactory } from '@nestjs/core';\nimport { VersioningType } from '@nestjs/common';",
        )
        .replace(
            "  app.setGlobalPrefix('api');",
            "  app.enableVersioning({ type: VersioningType.URI });",
        ),
    );
    p.write(
        "src/users/users.controller.ts",
        r#"
import { Controller, Get, Version } from '@nestjs/common';

@Controller({ path: 'users', version: ['1', '2'] })
export class UsersController {
  @Get()
  findAll() {}
}
"#,
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    // Both variants exist under collision-suffixed names (two requests want
    // the same handler name); together they cover v1 and v2.
    let mut urls: Vec<String> = std::fs::read_dir(p.root().join("bruno").join("Users"))
        .expect("folder")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .map(|name| request_url(&p, &format!("Users/{name}")))
        .collect();
    urls.sort();
    assert_eq!(
        urls,
        ["  url: {{baseUrl}}/v1/users", "  url: {{baseUrl}}/v2/users"]
    );
}

#[test]
fn a_non_uri_versioning_strategy_never_makes_url_segments() {
    let p = TestProject::new();
    p.write(
        "src/main.ts",
        &BOOTSTRAP_WITH_PREFIX.replace(
            "import { NestFactory } from '@nestjs/core';",
            "import { NestFactory } from '@nestjs/core';\nimport { VersioningType } from '@nestjs/common';",
        )
        .replace(
            "  app.setGlobalPrefix('api');",
            "  app.enableVersioning({ type: VersioningType.HEADER, header: 'X-Version' });",
        ),
    );
    p.write("src/users/users.controller.ts", &controller("users"));
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        request_url(&p, "Users/find-all.bru"),
        "  url: {{baseUrl}}/users"
    );
}

#[test]
fn a_configured_bootstrap_file_wins_over_the_convention() {
    let p = TestProject::new();
    p.write("src/main.ts", "export {}"); // conventional file without a prefix
    p.write("bootstrap.ts", BOOTSTRAP_WITH_PREFIX);
    p.write(
        "nestbruno.toml",
        "source = \"src\"\nbruno = \"bruno\"\nbootstrap = \"bootstrap.ts\"\n",
    );
    p.write("src/users/users.controller.ts", &controller("users"));
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        request_url(&p, "Users/find-all.bru"),
        "  url: {{baseUrl}}/api/users"
    );
}
