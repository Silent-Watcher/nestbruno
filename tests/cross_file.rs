//! Acceptance tests for cross-file DTO resolution through the complete
//! pipeline: real NestJS-shaped projects, the real binary, the actual `.bru`
//! output (discovery -> parser -> cross-file resolution -> route IR ->
//! generator -> writer).
// Scoped allowance (this file only): the harness panics with a message when
// the project under test cannot be set up, the right failure mode for test
// infrastructure; clippy's test exemption does not cover integration files.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{TestProject, stderr, stdout, tree};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The command output with column alignment collapsed, for count assertions.
fn flat(output: &std::process::Output) -> String {
    stdout(output)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn controller(import_from: &str, dto_type: &str) -> String {
    format!(
        "import {{ Controller, Post, Body }} from '@nestjs/common';\n\
         import {{ {dto_type} }} from '{import_from}';\n\
         \n\
         @Controller('users')\n\
         export class UsersController {{\n\
         \x20 @Post()\n\
         \x20 create(@Body() dto: {dto_type}) {{}}\n\
         }}\n"
    )
}

fn body_of(p: &TestProject, request: &str) -> String {
    let content = text(&std::fs::read(p.root().join("bruno").join(request)).expect("request"));
    let start = content.find("body:json {").expect("body block") + "body:json {".len();
    let rest = &content[start..];
    let end = rest.find("\n}").expect("body end");
    rest[..end].trim().to_owned()
}

fn docs_of(p: &TestProject, request: &str) -> String {
    let content = text(&std::fs::read(p.root().join("bruno").join(request)).expect("request"));
    let start = content.find("docs {").expect("docs block");
    content[start..].to_owned()
}

/// One controller importing a DTO class from `src/dto/create-user.dto.ts`,
/// written through `write_specifier` (which controls the import form).
fn project_with_class_dto(write_specifier: impl Fn(&TestProject)) -> TestProject {
    let p = TestProject::new();
    p.write(
        "src/dto/create-user.dto.ts",
        "export class CreateUserDto {\n  name: string;\n  email: string;\n}\n",
    );
    write_specifier(&p);
    p
}

fn write_relative_controller(p: &TestProject) {
    p.write(
        "src/users.controller.ts",
        &controller("./dto/create-user.dto", "CreateUserDto"),
    );
}

// ------------------------------------------------------------ the pipeline

#[test]
fn a_cross_file_class_dto_becomes_a_body_skeleton() {
    let p = project_with_class_dto(write_relative_controller);
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(flat(&output).contains("Created: 1"), "{}", flat(&output));
    assert_eq!(
        body_of(&p, "Users/create.bru"),
        "{\n    \"name\": null,\n    \"email\": null\n  }"
    );
    let docs = docs_of(&p, "Users/create.bru");
    assert!(docs.contains("@Body(): type CreateUserDto"), "{docs}");
    assert!(docs.contains("Source: src/users.controller.ts"), "{docs}");
    assert!(!docs.contains("fields not inferred"), "{docs}");
}

#[test]
fn an_interface_dto_becomes_a_body_skeleton() {
    let p = project_with_class_dto(|p| {
        std::fs::write(
            p.root().join("src/dto/create-user.dto.ts"),
            "export interface CreateUserDto {\n  name: string;\n  email: string;\n}\n",
        )
        .expect("dto");
        p.write(
            "src/users.controller.ts",
            &controller("./dto/create-user.dto", "CreateUserDto"),
        );
    });
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(body_of(&p, "Users/create.bru").contains("\"email\""));
}

#[test]
fn a_zod_infer_dto_becomes_a_body_skeleton() {
    let p = TestProject::new();
    p.write(
        "src/dto/create-user.schema.ts",
        "import { z } from 'zod';\n\
         export const createUserSchema = z.object({ name: z.string(), email: z.string() });\n\
         export type CreateUserDto = z.infer<typeof createUserSchema>;\n",
    );
    p.write(
        "src/users.controller.ts",
        &controller("./dto/create-user.schema", "CreateUserDto"),
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        body_of(&p, "Users/create.bru"),
        "{\n    \"name\": null,\n    \"email\": null\n  }"
    );
}

#[test]
fn nodenext_js_specifiers_resolve_end_to_end() {
    let p = project_with_class_dto(|p| {
        p.write(
            "src/users.controller.ts",
            &controller("./dto/create-user.dto.js", "CreateUserDto"),
        );
    });
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(body_of(&p, "Users/create.bru").contains("\"email\""),);
}

#[test]
fn package_imports_subpaths_resolve_end_to_end() {
    let p = TestProject::new();
    p.write(
        "package.json",
        "{\n  \"name\": \"app\",\n  \"imports\": {\n    \"#src/*\": \"./src/*\"\n  }\n}\n",
    );
    p.write(
        "src/dto/create-user.dto.ts",
        "export class CreateUserDto {\n  name: string;\n  email: string;\n}\n",
    );
    p.write(
        "src/users.controller.ts",
        &controller("#src/dto/create-user.dto.js", "CreateUserDto"),
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(body_of(&p, "Users/create.bru").contains("\"email\""));
}

#[test]
fn a_multi_hop_reexport_chain_resolves_end_to_end() {
    let p = TestProject::new();
    p.write(
        "src/dto/create-user.dto.ts",
        "export class CreateUserDto {\n  name: string;\n}\n",
    );
    p.write(
        "src/dto/index.ts",
        "export { CreateUserDto } from './create-user.dto';\n",
    );
    p.write("src/barrel.ts", "export { CreateUserDto } from './dto';\n");
    p.write(
        "src/barrel2.ts",
        "export { CreateUserDto } from './barrel';\n",
    );
    p.write(
        "src/users.controller.ts",
        &controller("./barrel2", "CreateUserDto"),
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(body_of(&p, "Users/create.bru").contains("\"name\""));
}

// ------------------------------------------------------------ reconciliation

#[test]
fn a_dto_change_updates_the_request_in_place() {
    let p = project_with_class_dto(write_relative_controller);
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let before = tree(&p.root().join("bruno"));

    p.write(
        "src/dto/create-user.dto.ts",
        "export class CreateUserDto {\n  name: string;\n  email: string;\n  age: number;\n}\n",
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = flat(&output);
    assert!(
        out.contains("Updated: 1") && out.contains("Created: 0") && out.contains("Deleted: 0"),
        "{out}"
    );
    assert!(body_of(&p, "Users/create.bru").contains("\"age\""));
    // The request kept its identity: same file name, same route.
    let after = tree(&p.root().join("bruno"));
    let mut names_before = names_of(&before);
    let mut names_after = names_of(&after);
    names_before.sort();
    names_after.sort();
    assert_eq!(
        names_before, names_after,
        "no request was renamed or recreated"
    );
}

#[test]
fn an_unresolvable_dto_stays_unknown_with_an_explanation() {
    let p = TestProject::new();
    // The import points at a module that does not exist.
    p.write(
        "src/users.controller.ts",
        &controller("./dto/missing", "GhostDto"),
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(body_of(&p, "Users/create.bru"), "{}");
    let docs = docs_of(&p, "Users/create.bru");
    assert!(docs.contains("fields not inferred"), "{docs}");
}

// ------------------------------------------------------------ determinism

#[test]
fn repeated_syncs_are_byte_identical_with_cross_file_dtos() {
    let p = project_with_class_dto(write_relative_controller);
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let first = tree(&p.root().join("bruno"));

    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(flat(&output).contains("Unchanged: 1"), "{}", flat(&output));
    assert_eq!(
        tree(&p.root().join("bruno")),
        first,
        "byte-identical collection"
    );
}

fn names_of(t: &[(String, Vec<u8>)]) -> Vec<String> {
    t.iter().map(|(n, _)| n.clone()).collect()
}

// ------------------------------------------------- query/param DTO resolution

#[test]
fn a_cross_file_query_dto_becomes_disabled_query_parameters() {
    let p = TestProject::new();
    p.write(
        "src/dto/find-users.query.ts",
        "export class FindUsersQuery {\n  page: number;\n  limit: number;\n  search: string;\n}\n",
    );
    p.write(
        "src/users.controller.ts",
        "import { Controller, Get, Query } from '@nestjs/common';\n\
         import { FindUsersQuery } from './dto/find-users.query';\n\
         @Controller('users')\n\
         export class UsersController {\n\
         \x20 @Get()\n\
         \x20 findAll(@Query() query: FindUsersQuery) {}\n\
         }\n",
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let content = text(&std::fs::read(p.root().join("bruno/Users/find-all.bru")).expect("req"));
    assert!(content.contains("params:query {"), "{content}");
    assert!(content.contains("~page:"), "{content}");
    assert!(content.contains("~limit:"), "{content}");
    assert!(content.contains("~search:"), "{content}");
    assert!(!content.contains("fields not inferred"), "{content}");
}

#[test]
fn named_and_dto_query_parameters_deduplicate() {
    let p = TestProject::new();
    p.write(
        "src/dto/find-users.query.ts",
        "export class FindUsersQuery {\n  page: number;\n  limit: number;\n}\n",
    );
    p.write(
        "src/users.controller.ts",
        "import { Controller, Get, Query } from '@nestjs/common';\n\
         import { FindUsersQuery } from './dto/find-users.query';\n\
         @Controller('users')\n\
         export class UsersController {\n\
         \x20 @Get()\n\
         \x20 findAll(@Query() query: FindUsersQuery, @Query('page') page: string) {}\n\
         }\n",
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let content = text(&std::fs::read(p.root().join("bruno/Users/find-all.bru")).expect("req"));
    assert_eq!(content.matches("~page:").count(), 1, "{content}");
    assert!(content.matches("~limit:").count() == 1, "{content}");
}

#[test]
fn a_param_dto_never_invents_path_placeholders() {
    let p = TestProject::new();
    p.write(
        "src/dto/find-post.params.ts",
        "export class FindPostParams {\n  userId: string;\n  postId: string;\n  ghost: string;\n}\n",
    );
    p.write(
        "src/posts.controller.ts",
        "import { Controller, Get, Param } from '@nestjs/common';\n\
         import { FindPostParams } from './dto/find-post.params';\n\
         @Controller('users/:userId/posts')\n\
         export class PostsController {\n\
         \x20 @Get(':postId')\n\
         \x20 findOne(@Param() params: FindPostParams) {}\n\
         }\n",
    );
    let output = p.run(&["sync"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let content = text(&std::fs::read(p.root().join("bruno/Posts/find-one.bru")).expect("req"));
    // URL is the source of truth for path parameters.
    assert!(content.contains("params:path {"), "{content}");
    assert!(content.contains("userId:"), "{content}");
    assert!(content.contains("postId:"), "{content}");
    // A DTO field absent from the URL is documented, never invented.
    assert!(
        content.contains("ghost"),
        "the extra DTO field is reported: {content}"
    );
    assert!(
        !content.contains("~ghost:"),
        "no invented query param: {content}"
    );
}
