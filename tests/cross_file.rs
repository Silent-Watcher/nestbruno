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
