//! Integration tests for synchronization: NestJS source -> parser -> IR ->
//! desired requests -> existing collection -> reconciliation -> filesystem,
//! through the real binary.

mod common;

use std::fs;

use common::{TestProject, stderr, stdout, tree};

const USERS: &str = "\
@Controller('users')
export class UsersController {
  @Get()
  findAll(@Query('page') page: number) {}

  @Get(':id')
  findOne(@Param('id') id: string) {}

  @Post()
  create(@Body() dto: CreateUserDto) {}
}
";

fn project(source: &str) -> TestProject {
    let p = TestProject::new();
    p.write("src/users/users.controller.ts", source);
    p
}

fn bru(p: &TestProject, rel: &str) -> String {
    common::read_text(&p.root().join("bruno").join(rel))
}

fn sync(p: &TestProject) -> std::process::Output {
    p.run(&["sync"])
}

fn summary(output: &std::process::Output) -> String {
    stdout(output)
        .lines()
        .filter(|l| l.contains(":   "))
        .collect::<Vec<_>>()
        .join("|")
}

fn bruno_tree(p: &TestProject) -> Vec<(String, Vec<u8>)> {
    tree(&p.root().join("bruno"))
}

// ------------------------------------------------------------ the basic cycle

#[test]
fn a_new_project_gets_all_its_requests() {
    let p = project(USERS);
    let output = sync(&p);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output).replace('\\', "/");
    assert!(out.contains("CREATE    bruno/Users/find-all.bru"), "{out}");
    assert!(out.contains("CREATE    bruno/Users/create.bru"), "{out}");
    assert_eq!(
        summary(&output),
        "Created:     3|Updated:     0|Unchanged:   0|Deleted:     0|Preserved:   0|Conflicts:   0"
    );
    assert!(p.root().join("bruno/bruno.json").is_file());
    assert!(bru(&p, "Users/find-one.bru").contains("NestBruno: v1 id="));
}

#[test]
fn running_sync_again_changes_nothing() {
    let p = project(USERS);
    sync(&p);
    let before = bruno_tree(&p);
    let output = sync(&p);
    assert!(output.status.success());
    assert_eq!(
        summary(&output),
        "Created:     0|Updated:     0|Unchanged:   3|Deleted:     0|Preserved:   0|Conflicts:   0"
    );
    assert!(!stdout(&output).contains("CREATE") && !stdout(&output).contains("UPDATE"));
    assert_eq!(bruno_tree(&p), before, "byte for byte");
}

#[test]
fn sync_is_deterministic_across_directories() {
    let a = project(USERS);
    let b = project(USERS);
    assert_eq!(stdout(&sync(&a)), stdout(&sync(&b)));
    assert_eq!(bruno_tree(&a), bruno_tree(&b));
}

#[test]
fn a_changed_route_updates_its_request_in_place() {
    let p = project(USERS);
    sync(&p);
    p.write_over(
        "src/users/users.controller.ts",
        &USERS.replace(
            "@Query('page') page: number",
            "@Query('page') page: number, @Query('limit') limit: number",
        ),
    );
    let output = sync(&p);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output)
            .replace('\\', "/")
            .contains("UPDATE    bruno/Users/find-all.bru")
    );
    assert!(bru(&p, "Users/find-all.bru").contains("~limit: "));
    assert_eq!(
        summary(&output),
        "Created:     0|Updated:     1|Unchanged:   2|Deleted:     0|Preserved:   0|Conflicts:   0"
    );
    // ... and the next run is quiet.
    assert!(summary(&sync(&p)).contains("Unchanged:   3"));
}

#[test]
fn a_removed_route_deletes_its_unmodified_request_and_an_empty_folder() {
    let p = project(USERS);
    sync(&p);
    p.write_over("src/users/users.controller.ts", "@Controller('users') export class UsersController { @Get() findAll() {} @Post() create() {} }");
    let output = sync(&p);
    assert!(
        stdout(&output)
            .replace('\\', "/")
            .contains("DELETE    bruno/Users/find-one.bru")
    );
    assert!(!p.root().join("bruno/Users/find-one.bru").exists());

    p.write_over("src/users/users.controller.ts", "export class Nothing {}");
    let output = sync(&p);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        !p.root().join("bruno/Users").exists(),
        "the empty folder is removed"
    );
    assert!(p.root().join("bruno/bruno.json").is_file());
}

#[test]
fn a_removed_controller_file_deletes_its_requests() {
    let p = project(USERS);
    p.write(
        "src/auth/auth.controller.ts",
        "@Controller('auth') export class AuthController { @Post('login') login() {} }",
    );
    sync(&p);
    fs::remove_file(p.root().join("src/auth/auth.controller.ts")).unwrap();
    let output = sync(&p);
    assert!(
        stdout(&output)
            .replace('\\', "/")
            .contains("DELETE    bruno/Auth/login.bru")
    );
    assert!(!p.root().join("bruno/Auth").exists());
    assert!(p.root().join("bruno/Users/find-all.bru").is_file());
}

// ------------------------------------------------------------ user work is protected

#[test]
fn a_user_modified_request_is_a_conflict_and_is_never_overwritten() {
    let p = project(USERS);
    sync(&p);
    let edited = bru(&p, "Users/find-all.bru").replace("auth: inherit", "auth: bearer");
    p.write_over("bruno/Users/find-all.bru", &edited);
    p.write_over(
        "src/users/users.controller.ts",
        &USERS.replace(
            "@Get()\n  findAll(@Query('page') page: number)",
            "@Get('all')\n  findAll(@Query('page') page: number)",
        ),
    );

    let output = sync(&p);
    assert_eq!(
        output.status.code(),
        Some(4),
        "conflicts are reported through the exit code"
    );
    let out = stdout(&output).replace('\\', "/");
    assert!(
        out.contains("CONFLICT  bruno/Users/find-all.bru: the generated request was modified"),
        "{out}"
    );
    assert!(out.contains("what to do:"), "{out}");
    assert!(out.contains("Conflicts:   1"), "{out}");
    assert_eq!(bru(&p, "Users/find-all.bru"), edited);
    // Everything else was still synchronized.
    assert!(!summary(&output).contains("Unchanged:   0"));
}

#[test]
fn a_modified_request_of_an_unchanged_route_is_simply_left_alone() {
    let p = project(USERS);
    sync(&p);
    let edited = format!(
        "{}\nscript:pre-request {{\n  bru.setVar('x', 1);\n}}\n",
        bru(&p, "Users/find-all.bru")
    );
    p.write_over("bruno/Users/find-all.bru", &edited);
    for _ in 0..2 {
        let output = sync(&p);
        assert!(
            output.status.success(),
            "no conflict, nothing to merge: {}",
            stdout(&output)
        );
        assert_eq!(
            summary(&output),
            "Created:     0|Updated:     0|Unchanged:   3|Deleted:     0|Preserved:   0|Conflicts:   0"
        );
        assert_eq!(bru(&p, "Users/find-all.bru"), edited);
    }
}

#[test]
fn reordering_requests_in_bruno_is_not_a_modification() {
    let p = project(USERS);
    sync(&p);
    let reordered = bru(&p, "Users/find-one.bru").replace("seq: 2", "seq: 17");
    p.write_over("bruno/Users/find-one.bru", &reordered);
    p.write_over(
        "src/users/users.controller.ts",
        &USERS.replace(
            "@Param('id') id: string",
            "@Param('id') id: string, @Query('with') w: string",
        ),
    );
    let output = sync(&p);
    assert!(output.status.success(), "{}", stdout(&output));
    assert!(
        stdout(&output)
            .replace('\\', "/")
            .contains("UPDATE    bruno/Users/find-one.bru")
    );
    let updated = bru(&p, "Users/find-one.bru");
    assert!(
        updated.contains("seq: 17"),
        "the user's order survives the update"
    );
    assert!(updated.contains("~with: "));
}

#[test]
fn a_modified_request_of_a_removed_route_is_kept_and_reported() {
    let p = project(USERS);
    sync(&p);
    let edited = bru(&p, "Users/find-one.bru").replace("auth: inherit", "auth: bearer");
    p.write_over("bruno/Users/find-one.bru", &edited);
    p.write_over("src/users/users.controller.ts", "@Controller('users') export class UsersController { @Get() findAll(@Query('page') page: number) {} @Post() create(@Body() dto: D) {} }");
    let output = sync(&p);
    assert_eq!(output.status.code(), Some(4));
    assert!(
        stdout(&output)
            .contains("the route no longer exists, but the generated request was modified")
    );
    assert_eq!(bru(&p, "Users/find-one.bru"), edited);
}

#[test]
fn unknown_bruno_files_are_preserved_and_counted() {
    let p = project(USERS);
    p.write(
        "bruno/environments/local.bru",
        "vars {\n  baseUrl: http://localhost:3000\n}\n",
    );
    p.write("bruno/Users/folder.bru", "meta {\n  name: Users\n}\n");
    p.write("bruno/Handwritten/login.bru", "meta {\n  name: Login\n  type: http\n  seq: 1\n}\n\npost {\n  url: https://example.com\n  body: none\n  auth: none\n}\n");
    let before: Vec<_> = bruno_tree(&p);
    let output = sync(&p);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Preserved:   3"),
        "{}",
        stdout(&output)
    );
    for (name, content) in before {
        assert_eq!(
            fs::read(p.root().join("bruno").join(&name)).unwrap(),
            content,
            "{name}"
        );
    }
    // Even after every route is gone.
    p.write_over("src/users/users.controller.ts", "export class N {}");
    sync(&p);
    assert!(p.root().join("bruno/Handwritten/login.bru").is_file());
    assert!(p.root().join("bruno/environments/local.bru").is_file());
    assert!(p.root().join("bruno/Users/folder.bru").is_file());
}

#[test]
fn a_new_request_never_overwrites_a_hand_written_file_at_its_path() {
    let p = project(USERS);
    p.write("bruno/Users/find-all.bru", "my own request\n");
    let output = sync(&p);
    assert_eq!(output.status.code(), Some(4));
    assert!(
        stdout(&output)
            .contains("a different file already exists where this request would be created")
    );
    assert_eq!(bru(&p, "Users/find-all.bru"), "my own request\n");
    assert!(
        p.root().join("bruno/Users/find-one.bru").is_file(),
        "the other requests are created"
    );
}

// ------------------------------------------------------------ renames

#[test]
fn renaming_a_handler_updates_the_request_in_place_without_duplicates() {
    let p = project(USERS);
    sync(&p);
    p.write_over(
        "src/users/users.controller.ts",
        &USERS.replace("findOne(", "getUser("),
    );
    let output = sync(&p);
    assert!(output.status.success(), "{}", stdout(&output));
    assert_eq!(
        summary(&output),
        "Created:     0|Updated:     1|Unchanged:   2|Deleted:     0|Preserved:   0|Conflicts:   0"
    );
    assert!(bru(&p, "Users/find-one.bru").contains("UsersController.getUser"));
    assert_eq!(
        bruno_tree(&p).len(),
        4,
        "bruno.json + 3 requests, no duplicate"
    );
}

#[test]
fn moving_a_controller_file_updates_in_place_without_duplicates() {
    let p = project(USERS);
    sync(&p);
    fs::create_dir_all(p.root().join("src/moved")).unwrap();
    fs::rename(
        p.root().join("src/users/users.controller.ts"),
        p.root().join("src/moved/users.controller.ts"),
    )
    .unwrap();
    let output = sync(&p);
    assert!(output.status.success(), "{}", stdout(&output));
    assert!(
        summary(&output).contains("Updated:     3"),
        "{}",
        summary(&output)
    );
    assert_eq!(bruno_tree(&p).len(), 4);
    assert!(bru(&p, "Users/create.bru").contains("Source: src/moved/users.controller.ts"));
}

// ------------------------------------------------------------ duplicates (critical)

#[test]
fn duplicate_routes_in_the_source_block_all_writes() {
    let p = project(
        "@Controller('users') export class UsersController { @Get() list() {} @Get() list() {} @Post() create() {} }",
    );
    let output = sync(&p);
    assert_eq!(output.status.code(), Some(4));
    let out = stdout(&output);
    assert!(
        out.contains("several routes in the source have the same identity"),
        "{out}"
    );
    assert!(
        stderr(&output).contains("nothing was written"),
        "{}",
        stderr(&output)
    );
    assert!(!p.root().join("bruno").exists());
}

#[test]
fn a_cloned_request_with_the_same_stamp_blocks_all_writes() {
    let p = project(USERS);
    sync(&p);
    let copy = bru(&p, "Users/find-all.bru");
    p.write("bruno/Users/find-all-copy.bru", &copy);
    p.write_over(
        "src/users/users.controller.ts",
        &USERS.replace("@Query('page') page: number", "@Query('limit') l: number"),
    );
    let before = bruno_tree(&p);

    let output = sync(&p);
    assert_eq!(output.status.code(), Some(4));
    let out = stdout(&output);
    assert_eq!(
        out.matches("several request files claim the same route")
            .count(),
        2,
        "{out}"
    );
    assert!(out.contains("delete the 'NestBruno:' line"), "{out}");
    assert_eq!(
        bruno_tree(&p),
        before,
        "nothing was written, not even unrelated updates"
    );
}

// ------------------------------------------------------------ dry run

#[test]
fn dry_run_reports_exactly_what_a_real_run_does_and_writes_nothing() {
    let p = project(USERS);
    let dry = p.run(&["sync", "--dry-run"]);
    assert!(dry.status.success(), "{}", stderr(&dry));
    assert!(
        stdout(&dry).contains("CREATE    bruno/Users/find-all.bru")
            || stdout(&dry)
                .replace('\\', "/")
                .contains("CREATE    bruno/Users/find-all.bru")
    );
    assert!(stdout(&dry).trim_end().ends_with("No files were written."));
    assert!(!p.root().join("bruno").exists());

    let real = sync(&p);
    let strip = |s: String| s.replace("No files were written.\n", "");
    assert_eq!(strip(stdout(&dry)), strip(stdout(&real)));

    // Updates and deletes too, with user edits in the mix.
    p.write_over("src/users/users.controller.ts", "@Controller('users') export class UsersController { @Get() findAll(@Query('q') q: string) {} }");
    let before = bruno_tree(&p);
    let dry = p.run(&["sync", "--dry-run"]);
    let out = stdout(&dry).replace('\\', "/");
    assert!(
        out.contains("UPDATE    bruno/Users/find-all.bru")
            && out.contains("DELETE    bruno/Users/find-one.bru"),
        "{out}"
    );
    assert_eq!(bruno_tree(&p), before);
    let real = sync(&p);
    assert_eq!(strip(stdout(&dry)), strip(stdout(&real)));
}

#[test]
fn dry_run_output_is_deterministic_and_lists_conflicts() {
    let p = project(USERS);
    sync(&p);
    p.write_over(
        "bruno/Users/find-all.bru",
        &bru(&p, "Users/find-all.bru").replace("body: none", "body: json"),
    );
    p.write_over(
        "src/users/users.controller.ts",
        &USERS.replace("@Query('page') page: number", ""),
    );
    let first = p.run(&["sync", "--dry-run"]);
    let second = p.run(&["sync", "--dry-run"]);
    assert_eq!(stdout(&first), stdout(&second));
    assert!(stdout(&first).contains("CONFLICT"));
    assert_eq!(first.status.code(), Some(4));
}

#[test]
fn verbose_lists_preserved_and_unchanged_files() {
    let p = project(USERS);
    p.write("bruno/mine.bru", "x");
    sync(&p);
    let output = p.run(&["sync", "-v"]);
    let out = stdout(&output).replace('\\', "/");
    assert!(
        out.contains("PRESERVE  bruno/mine.bru")
            && out.contains("UNCHANGED bruno/Users/find-all.bru"),
        "{out}"
    );
}

// ------------------------------------------------------------ incomplete source

#[test]
fn nothing_is_deleted_when_a_source_file_cannot_be_parsed() {
    let p = project(USERS);
    p.write(
        "src/auth/auth.controller.ts",
        "@Controller('auth') export class AuthController { @Post('login') login() {} }",
    );
    sync(&p);
    // The auth controller becomes unparsable (an editor mid-save, say).
    p.write_over(
        "src/auth/auth.controller.ts",
        "@Controller('auth') export class AuthController { @Post('login' login() {} }",
    );
    let output = sync(&p);
    assert_eq!(output.status.code(), Some(4));
    assert!(
        p.root().join("bruno/Auth/login.bru").is_file(),
        "its request must survive"
    );
    assert!(stdout(&output).contains("Deleted:     0"));
    assert!(stderr(&output).contains("src/auth/auth.controller.ts"));
    assert!(
        stderr(&output).contains("not deleted because source discovery or analysis was incomplete")
    );

    // Fixed again: back to normal.
    p.write_over(
        "src/auth/auth.controller.ts",
        "@Controller('auth') export class AuthController { @Post('login') login() {} }",
    );
    assert!(sync(&p).status.success());
}

#[test]
fn a_route_that_cannot_be_generated_does_not_delete_its_existing_request() {
    let p = project(USERS);
    sync(&p);
    // `:id` route becomes dynamic: still a route, but no request can be produced.
    p.write_over(
        "src/users/users.controller.ts",
        &USERS.replace("@Get(':id')", "@Get(buildPath())"),
    );
    let output = sync(&p);
    assert!(p.root().join("bruno/Users/find-one.bru").is_file());
    assert!(
        stdout(&output).contains("Deleted:     0"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("could be generated"),
        "{}",
        stderr(&output)
    );
}

// ------------------------------------------------------------ legacy and invalid files

#[test]
fn unstamped_files_from_generate_in_an_earlier_version_are_adopted_only_when_identical() {
    let p = project(USERS);
    sync(&p);
    // Simulate a file written before stamps existed: the exact generated text, minus the stamp.
    let strip = |text: String| {
        text.lines()
            .filter(|l| !l.contains("NestBruno:"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };
    let identical = strip(bru(&p, "Users/find-all.bru"));
    let edited = strip(bru(&p, "Users/find-one.bru")).replace("auth: inherit", "auth: bearer");
    p.write_over("bruno/Users/find-all.bru", &identical);
    p.write_over("bruno/Users/find-one.bru", &edited);

    let output = sync(&p);
    let out = stdout(&output).replace('\\', "/");
    assert!(out.contains("UPDATE    bruno/Users/find-all.bru"), "{out}");
    assert!(out.contains("CONFLICT  bruno/Users/find-one.bru"), "{out}");
    assert!(bru(&p, "Users/find-all.bru").contains("NestBruno: v1"));
    assert_eq!(bru(&p, "Users/find-one.bru"), edited);
}

#[test]
fn invalid_files_produce_diagnostics_and_are_never_touched() {
    let p = project(USERS);
    p.write("bruno/a-binary.bru", "x\0y");
    p.write(
        "bruno/Users/broken-stamp.bru",
        "docs {\n  NestBruno: v1 id=oops\n}\n",
    );
    fs::write(p.root().join("bruno/not-utf8.bru"), [0xff, 0xfe, 0xfd]).unwrap();
    let output = sync(&p);
    assert_eq!(output.status.code(), Some(4));
    let err = stderr(&output).replace('\\', "/");
    assert!(
        err.contains("warning: bruno/a-binary.bru: binary content; left untouched"),
        "{err}"
    );
    assert!(
        err.contains("warning: bruno/not-utf8.bru: not valid UTF-8; left untouched"),
        "{err}"
    );
    assert!(stdout(&output).contains("the NestBruno metadata line is invalid"));
    assert_eq!(
        fs::read(p.root().join("bruno/not-utf8.bru")).unwrap(),
        [0xff, 0xfe, 0xfd]
    );
    assert_eq!(bru(&p, "a-binary.bru"), "x\0y");
    assert!(p.root().join("bruno/Users/find-all.bru").is_file());
}

// ------------------------------------------------------------ security

#[test]
fn hostile_source_cannot_make_sync_write_or_delete_outside_the_collection() {
    let p = TestProject::new();
    p.write(
        "src/evil.controller.ts",
        r#"@Controller('x') export class EvilController {
  @Get('a\nscript:pre-request {\n  evil()\n}\n') newline() {}
  @Get('/{{process.env.SECRET}}') env() {}
  @Get('ok') ['../../../escape']() {}
  @Get('fine') '../../../../etc/passwd'() {}
}"#,
    );
    p.write("outside.txt", "precious");
    let before: Vec<_> = tree(p.root())
        .into_iter()
        .filter(|(n, _)| !n.starts_with("bruno/"))
        .collect();
    assert!(sync(&p).status.success());
    let after: Vec<_> = tree(p.root())
        .into_iter()
        .filter(|(n, _)| !n.starts_with("bruno/"))
        .collect();
    assert_eq!(before, after);
    for (name, content) in bruno_tree(&p) {
        let text = String::from_utf8_lossy(&content);
        assert!(
            !text.contains("evil()") && !text.contains("process.env"),
            "{name}"
        );
        assert!(!name.contains(".."), "{name}");
    }
    // Sync it again after removing the routes: still nothing outside is touched.
    p.write_over("src/evil.controller.ts", "export class Nothing {}");
    sync(&p);
    assert_eq!(
        fs::read_to_string(p.root().join("outside.txt")).unwrap(),
        "precious"
    );
}

#[test]
fn a_forged_stamp_cannot_be_used_to_delete_a_modified_file() {
    // Someone copies a stamp onto a file with different content: it hashes
    // differently, so it counts as modified and is never deleted.
    let p = project(USERS);
    sync(&p);
    let stamp_line = bru(&p, "Users/create.bru")
        .lines()
        .find(|l| l.contains("NestBruno:"))
        .unwrap()
        .to_owned();
    p.write(
        "bruno/Users/forged.bru",
        &format!("meta {{\n  name: Mine\n}}\n\ndocs {{\n  important notes\n{stamp_line}\n}}\n"),
    );
    p.write_over("src/users/users.controller.ts", "export class N {}");
    let output = sync(&p);
    assert!(p.root().join("bruno/Users/forged.bru").is_file());
    assert_eq!(output.status.code(), Some(4));
    assert!(stdout(&output).contains("several request files claim the same route"));
}

#[cfg(unix)]
mod symlinks {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn a_symlinked_collection_directory_is_refused() {
        let p = project(USERS);
        let outside = TestProject::new();
        symlink(outside.root(), p.root().join("bruno")).unwrap();
        let output = sync(&p);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("symlink"), "{}", stderr(&output));
        assert!(tree(outside.root()).is_empty());
    }

    #[test]
    fn symlinked_requests_are_never_followed_deleted_or_replaced() {
        let p = project(USERS);
        sync(&p);
        let outside = TestProject::new();
        let victim = outside.write("victim.bru", "precious");
        fs::remove_file(p.root().join("bruno/Users/create.bru")).unwrap();
        symlink(&victim, p.root().join("bruno/Users/create.bru")).unwrap();
        p.write_over("src/users/users.controller.ts", "export class N {}");
        let output = sync(&p);
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        assert!(
            fs::symlink_metadata(p.root().join("bruno/Users/create.bru"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(output.status.code().is_some());
    }
}

// ------------------------------------------------------------ Phase 9: body shape updates in place

#[test]
fn adding_a_dto_to_a_body_parameter_updates_in_place() {
    // Start with @Body() without a DTO -> Unknown -> {}
    let p = project(
        "@Controller('users')\nexport class UsersController {\n  @Post() create(@Body() dto) {}\n}",
    );
    sync(&p);
    let created = bru(&p, "Users/create.bru");
    assert!(created.contains("body:json {\n  {}\n}"));
    assert!(created.contains("@Body(): whole object, fields not inferred"));

    // Now add a DTO with properties -> Fields -> skeleton with fields
    p.write_over(
        "src/users/users.controller.ts",
        "@Controller('users')\nexport class UsersController {\n  @Post() create(@Body() dto: CreateUserDto) {}\n}\nclass CreateUserDto {\n  name: string;\n  email: string;\n}",
    );
    let output = sync(&p);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output).replace('\\', "/");
    assert!(out.contains("UPDATE    bruno/Users/create.bru"), "{out}");

    let updated = bru(&p, "Users/create.bru");
    assert!(updated.contains("body:json {\n  {\n    \"name\": null,\n    \"email\": null\n  }\n}"));
    // The "fields not inferred" note should be gone since we have DTO fields
    assert!(!updated.contains("@Body(): whole object, fields not inferred"));
}

#[test]
fn removing_dto_fields_updates_in_place() {
    let p = project(
        "@Controller('users')\nexport class UsersController {\n  @Post() create(@Body() dto: CreateUserDto) {}\n}\nclass CreateUserDto {\n  name: string;\n  email: string;\n  age: number;\n}",
    );
    sync(&p);
    let created = bru(&p, "Users/create.bru");
    assert!(created.contains("\"name\": null"));
    assert!(created.contains("\"email\": null"));
    assert!(created.contains("\"age\": null"));

    // Remove one property
    p.write_over(
        "src/users/users.controller.ts",
        "@Controller('users')\nexport class UsersController {\n  @Post() create(@Body() dto: CreateUserDto) {}\n}\nclass CreateUserDto {\n  name: string;\n  email: string;\n}",
    );
    let output = sync(&p);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output).replace('\\', "/");
    assert!(out.contains("UPDATE    bruno/Users/create.bru"), "{out}");

    let updated = bru(&p, "Users/create.bru");
    assert!(updated.contains("\"name\": null"));
    assert!(updated.contains("\"email\": null"));
    assert!(!updated.contains("\"age\": null"));
}

#[test]
fn changing_dto_name_treats_as_unknown_not_delete_create() {
    // DTO name change should just update the body shape, not delete/recreate
    let p = project(
        "@Controller('users')\nexport class UsersController {\n  @Post() create(@Body() dto: OldDto) {}\n}\nclass OldDto {\n  name: string;\n}",
    );
    sync(&p);
    let created = bru(&p, "Users/create.bru");
    assert!(created.contains("\"name\": null"));

    p.write_over(
        "src/users/users.controller.ts",
        "@Controller('users')\nexport class UsersController {\n  @Post() create(@Body() dto: NewDto) {}\n}\nclass NewDto {\n  name: string;\n  email: string;\n}",
    );
    let output = sync(&p);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output).replace('\\', "/");
    // Should update in place, not delete+create
    assert!(out.contains("UPDATE    bruno/Users/create.bru"), "{out}");
    assert!(!out.contains("DELETE"));
    assert!(!out.contains("CREATE    bruno/Users/create.bru"));

    let updated = bru(&p, "Users/create.bru");
    assert!(updated.contains("\"name\": null"));
    assert!(updated.contains("\"email\": null"));
}

// ------------------------------------------------------------ scale

#[test]
fn a_large_collection_synchronizes_correctly_and_idempotently() {
    let p = TestProject::new();
    let source = |extra: &str| {
        let mut out = String::new();
        for m in 0..20 {
            out.push_str(&format!(
                "@Controller('m{m}') export class M{m}Controller {{\n"
            ));
            for i in 0..150 {
                out.push_str(&format!(
                    "  @Get('r{i}/:id') h{i}(@Query('q{i}{extra}') q: string) {{}}\n"
                ));
            }
            out.push_str("}\n");
        }
        out
    };
    p.write("src/all.controller.ts", &source(""));
    let first = sync(&p);
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(
        summary(&first).starts_with("Created:     3000"),
        "{}",
        summary(&first)
    );
    assert!(summary(&sync(&p)).contains("Unchanged:   3000"));

    p.write_over("src/all.controller.ts", &source("x"));
    let output = sync(&p);
    assert!(
        summary(&output).starts_with("Created:     0|Updated:     3000"),
        "{}",
        summary(&output)
    );
    assert!(summary(&sync(&p)).contains("Unchanged:   3000"));
}
