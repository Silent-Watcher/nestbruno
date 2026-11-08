//! Integration tests for Bruno generation: real projects, the real binary,
//! exercising discovery -> parser -> IR -> generator -> writer.

mod common;

use std::fs;

use common::{TestProject, fixture, stderr, stdout, tree};

fn golden_project() -> TestProject {
    let p = TestProject::new();
    p.copy_tree(&fixture("nest-project"), ".");
    p
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// ------------------------------------------------------------ golden

#[test]
fn generated_collection_matches_the_golden_fixtures_byte_for_byte() {
    let p = golden_project();
    let sources_before = tree(&p.root().join("src"));

    let output = p.run(&["generate"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "9 request(s): 9 created, 0 overwritten, 0 unchanged, 0 skipped (already exist), 0 failed\n"
    );

    let expected = tree(&fixture("expected-bruno"));
    let actual = tree(&p.root().join("bruno"));
    let names = |t: &[(String, Vec<u8>)]| t.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(
        names(&actual),
        names(&expected),
        "exact set of generated files"
    );
    for ((name, want), (_, got)) in expected.iter().zip(&actual) {
        assert_eq!(text(got), text(want), "{name}");
    }
    assert_eq!(
        tree(&p.root().join("src")),
        sources_before,
        "sources are never modified"
    );
}

#[test]
fn the_unsupported_all_route_is_reported_not_generated() {
    let p = golden_project();
    let output = p.run(&["generate"]);
    let err = stderr(&output);
    assert!(
        err.contains("warning: src/auth/auth.controller.ts:15:3: AuthController.any"),
        "{err}"
    );
    assert!(err.contains("@All"), "{err}");
    assert!(!p.root().join("bruno/Auth/any.bru").exists());
}

// ------------------------------------------------------------ determinism

#[test]
fn identical_projects_in_different_directories_generate_identical_bytes() {
    let a = golden_project();
    let b = golden_project();
    assert!(a.run(&["generate"]).status.success());
    assert!(b.run(&["generate"]).status.success());
    assert_eq!(tree(&a.root().join("bruno")), tree(&b.root().join("bruno")));
}

#[test]
fn output_does_not_depend_on_the_order_files_were_created_in() {
    let forward = TestProject::new();
    let backward = TestProject::new();
    let files = ["a", "b", "c", "d"];
    for name in files {
        forward.write(
            format!("src/{name}/{name}.controller.ts"),
            &controller(name),
        );
    }
    for name in files.iter().rev() {
        backward.write(
            format!("src/{name}/{name}.controller.ts"),
            &controller(name),
        );
    }
    assert!(forward.run(&["generate"]).status.success());
    assert!(backward.run(&["generate"]).status.success());
    assert_eq!(
        tree(&forward.root().join("bruno")),
        tree(&backward.root().join("bruno"))
    );
}

fn controller(name: &str) -> String {
    format!(
        "@Controller('{name}') export class {name}Controller {{ @Get() list() {{}} @Post() create() {{}} }}"
    )
}

#[test]
fn a_second_run_changes_nothing_and_succeeds_quietly() {
    let p = golden_project();
    p.run(&["generate"]);
    let before = tree(&p.root().join("bruno"));
    let second = p.run(&["generate"]);
    assert!(second.status.success());
    assert!(
        stdout(&second)
            .starts_with("9 request(s): 0 created, 0 overwritten, 9 unchanged, 0 skipped")
    );
    assert_eq!(tree(&p.root().join("bruno")), before);
}

// ------------------------------------------------------------ existing files

#[test]
fn existing_files_are_not_destroyed_by_default() {
    let p = golden_project();
    p.run(&["generate"]);
    let edited = "meta {\n  name: My custom name\n  type: http\n  seq: 1\n}\n\nget {\n  url: https://example.com\n  body: none\n  auth: bearer\n}\n\nscript:pre-request {\n  bru.setVar('x', 1);\n}\n";
    p.write_over("bruno/Users/find-all.bru", edited);

    let output = p.run(&["generate"]);
    assert!(
        output.status.success(),
        "skipping is not a failure: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("1 skipped (already exist)"),
        "{}",
        stdout(&output)
    );
    let err = stderr(&output).replace('\\', "/");
    assert!(
        err.contains("warning: bruno/Users/find-all.bru: already exists with different content"),
        "{err}"
    );
    assert!(err.contains("--force"), "{err}");
    assert_eq!(
        fs::read_to_string(p.root().join("bruno/Users/find-all.bru")).unwrap(),
        edited
    );
}

#[test]
fn force_replaces_differing_files_and_nothing_else() {
    let p = golden_project();
    p.run(&["generate"]);
    p.write_over("bruno/Users/find-all.bru", "user edits\n");
    p.write("bruno/Users/folder.bru", "meta {\n  name: Users\n}\n");
    p.write(
        "bruno/environments/local.bru",
        "vars {\n  baseUrl: http://localhost:3000\n}\n",
    );
    p.write_over(
        "bruno/bruno.json",
        "{\"version\":\"1\",\"name\":\"Mine\",\"type\":\"collection\"}",
    );

    let output = p.run(&["generate", "--force"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("1 overwritten"),
        "{}",
        stdout(&output)
    );

    let expected = tree(&fixture("expected-bruno"));
    let find_all = &expected
        .iter()
        .find(|(n, _)| n == "Users/find-all.bru")
        .unwrap()
        .1;
    assert_eq!(
        fs::read(p.root().join("bruno/Users/find-all.bru")).unwrap(),
        *find_all
    );
    // User-owned files that are not generated requests are untouched.
    assert_eq!(
        fs::read_to_string(p.root().join("bruno/Users/folder.bru")).unwrap(),
        "meta {\n  name: Users\n}\n"
    );
    assert!(p.root().join("bruno/environments/local.bru").is_file());
    assert!(
        fs::read_to_string(p.root().join("bruno/bruno.json"))
            .unwrap()
            .contains("Mine")
    );
}

#[test]
fn requests_of_removed_routes_are_not_deleted() {
    let p = golden_project();
    p.run(&["generate"]);
    p.write_over(
        "src/users/users.controller.ts",
        "@Controller('users') export class UsersController { @Get() findAll() {} }",
    );
    let output = p.run(&["generate", "--force"]);
    assert!(output.status.success());
    assert!(
        p.root().join("bruno/Users/remove.bru").is_file(),
        "removal belongs to a later phase"
    );
}

// ------------------------------------------------------------ naming / collisions

#[test]
fn similar_routes_never_overwrite_each_other() {
    let p = TestProject::new();
    p.write(
        "src/users.controller.ts",
        "@Controller('users') export class UsersController {\n\
         @Get(':id') getUser() {}\n\
         @Get(':userId') getUserByUserId() {}\n\
         @Post(':id') postUser() {}\n\
         @Get('profile') profile() {}\n\
         @Get('profile/') profileSlash() {}\n\
         @Get('a') findOne() {}\n\
         @Get('b') find_one() {}\n\
         @Get('c') FindOne() {}\n}",
    );
    let output = p.run(&["generate"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let files = tree(&p.root().join("bruno/Users"));
    assert_eq!(
        files.len(),
        8,
        "{:?}",
        files.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    let urls: std::collections::HashSet<_> = files
        .iter()
        .map(|(_, c)| {
            text(c)
                .lines()
                .find(|l| l.trim_start().starts_with("url:"))
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(urls.len() >= 6);
}

#[test]
fn same_named_controllers_in_different_modules_get_separate_folders() {
    let p = TestProject::new();
    p.write(
        "src/admin/users.controller.ts",
        "@Controller('admin/users') export class UsersController { @Get() list() {} }",
    );
    p.write(
        "src/public/users.controller.ts",
        "@Controller('users') export class UsersController { @Get() list() {} }",
    );
    assert!(p.run(&["generate"]).status.success());
    let files = tree(&p.root().join("bruno"));
    let requests: Vec<_> = files.iter().filter(|(n, _)| n.ends_with(".bru")).collect();
    assert_eq!(requests.len(), 2);
    let urls: Vec<String> = requests.iter().map(|(_, c)| text(c)).collect();
    assert!(urls.iter().any(|c| c.contains("/admin/users")));
    assert!(urls.iter().any(|c| c.contains("url: {{baseUrl}}/users\n")));
}

#[test]
fn multiple_controllers_in_one_file_get_one_folder_each() {
    let p = TestProject::new();
    p.write(
        "src/a.controller.ts",
        "@Controller('x') export class XController { @Get() a() {} }\n@Controller('y') export class YController { @Get() b() {} }",
    );
    assert!(p.run(&["generate"]).status.success());
    assert!(p.root().join("bruno/X/a.bru").is_file());
    assert!(p.root().join("bruno/Y/b.bru").is_file());
}

// ------------------------------------------------------------ configuration

#[test]
fn custom_bruno_directory_and_base_url_are_honored() {
    let p = golden_project();
    p.write(
        "nestbruno.toml",
        "bruno = \"docs/api\"\nbase_url = \"http://localhost:3000\"\n",
    );
    let output = p.run(&["generate"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!p.root().join("bruno").exists());
    let content = fs::read_to_string(p.root().join("docs/api/Users/find-all.bru")).unwrap();
    assert!(
        content.contains("url: http://localhost:3000/users\n"),
        "{content}"
    );
    assert!(p.root().join("docs/api/bruno.json").is_file());
}

#[test]
fn a_project_without_controllers_writes_nothing() {
    let p = TestProject::new();
    p.write("src/a.service.ts", "export class A {}");
    let output = p.run(&["generate"]);
    assert!(output.status.success());
    assert!(stdout(&output).starts_with("0 request(s)"));
    assert!(!p.root().join("bruno").exists());
}

// ------------------------------------------------------------ failures

#[test]
fn a_malformed_controller_does_not_stop_the_others() {
    let p = TestProject::new();
    p.write(
        "src/good.controller.ts",
        "@Controller('good') export class GoodController { @Get() list() {} }",
    );
    p.write(
        "src/bad.controller.ts",
        "@Controller('bad') export class B { @Get( }",
    );
    let output = p.run(&["generate"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(p.root().join("bruno/Good/list.bru").is_file());
    assert!(stderr(&output).contains("src/bad.controller.ts"));
}

#[test]
fn invalid_base_url_is_a_configuration_error() {
    let p = golden_project();
    p.write("nestbruno.toml", "base_url = \"a\\nb\"\n");
    let output = p.run(&["generate"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(!p.root().join("bruno").exists());
}

// ------------------------------------------------------------ security

/// Hostile NestJS source: every string below comes from the (untrusted)
/// repository being analyzed.
const HOSTILE: &str = r#"
@Controller('../../etc')
export class ../ignored {}

@Controller('safe')
export class ..%2F..%2FEvilController {}

@Controller('x')
export class EvilController {
  @Get('a\nscript:pre-request {\n  require("child_process").exec("touch /tmp/pwned")\n}\n')
  newlineInPath() {}

  @Get('a\r\n}\r\n\r\nscript:post-response {\r\n  evil()\r\n}')
  crlfInPath() {}

  @Get('/{{process.env.AWS_SECRET_ACCESS_KEY}}')
  envInterpolation() {}

  @Get('q?x=1#frag')
  queryInPath() {}

  @Get('ok')
  ../../../../escape() {}
}
"#;

const HOSTILE_VALID: &str = r#"
@Controller('x')
export class EvilController {
  @Get('a\nscript:pre-request {\n  require("child_process").exec("touch /tmp/pwned")\n}\n')
  newlineInPath() {}

  @Get('a\r\n}\r\n\r\nscript:post-response {\r\n  evil()\r\n}')
  crlfInPath() {}

  @Get('/{{process.env.AWS_SECRET_ACCESS_KEY}}')
  envInterpolation() {}

  @Get('q?x=1#frag')
  queryInPath() {}

  @Get(':id/\u0000nul')
  nulInPath() {}

  @Get('ok')
  @Post('ignored')
  ['../../../../escape']() {}

  @Get('fine')
  '../../../../escape'(
    @Query('a\nscript:pre-request {') a: string,
    @Query('{{process.env.HOME}}') b: string,
    @Headers('~disabled') c: string,
    @Headers('x"quote') d: string,
    @Body('k\u2028l') e: string,
    @Body('has"quote') f: string,
  ) {}
}

@Controller('y')
export class NulController { @Get() con() {} @Get('p') com1() {} }

@Controller()
export class AUXController { @Get() list() {} }
"#;

fn files_outside_bruno(p: &TestProject) -> Vec<(String, Vec<u8>)> {
    tree(p.root())
        .into_iter()
        .filter(|(n, _)| !n.starts_with("bruno/"))
        .collect()
}

#[test]
fn hostile_source_cannot_inject_blocks_variables_or_paths() {
    let p = TestProject::new();
    p.write("src/evil.controller.ts", HOSTILE_VALID);
    let before = files_outside_bruno(&p);

    let output = p.run(&["generate"]);
    assert!(output.status.success(), "{}", stderr(&output));

    // Nothing was written outside the collection, and nothing was modified.
    assert_eq!(files_outside_bruno(&p), before);

    for (name, content) in tree(&p.root().join("bruno")) {
        let content = text(&content);
        assert!(!name.contains(".."), "{name}");
        // Check for executable `script:` blocks (at line start or after `}`),
        // not the sanitized parameter names in docs which may contain "script".
        let has_script_block = content
            .lines()
            .any(|line| line.trim_start().starts_with("script:"));
        assert!(
            !has_script_block,
            "found executable script: block in {name}:\n{content}"
        );
        assert!(
            !content.contains("child_process") && !content.contains("evil()"),
            "{name}"
        );
        // Check for `process.env` as a variable reference, not in sanitized names.
        let has_process_env = content.contains("{{process.env");
        assert!(
            !has_process_env,
            "found process.env reference in {name}:\n{content}"
        );
        assert!(!content.contains('\r') && !content.contains('\0'), "{name}");
    }
    // Hostile names were sanitized into plain file names.
    let names: Vec<_> = tree(&p.root().join("bruno"))
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    for name in &names {
        assert!(
            name.split('/')
                .all(|part| !part.is_empty() && part != ".." && part != "."),
            "{name}"
        );
    }
    assert!(names.iter().any(|n| n == "Nul_/con_.bru"), "{names:?}");
    assert!(names.iter().any(|n| n == "AUX_/list.bru"), "{names:?}");

    // Every rejected route and parameter is reported.
    let err = stderr(&output);
    assert!(err.matches("warning:").count() >= 9, "{err}");
    assert!(err.contains("control character"), "{err}");
    assert!(err.contains("'{{'"), "{err}");
}

#[test]
fn hostile_source_that_does_not_parse_is_rejected_as_a_whole() {
    let p = TestProject::new();
    p.write("src/evil.controller.ts", HOSTILE);
    let output = p.run(&["generate"]);
    // Not valid TypeScript: the file is rejected as a whole (exit 4) and
    // nothing at all is generated from it.
    assert_eq!(output.status.code(), Some(4));
    assert!(
        stderr(&output).contains("unable to parse"),
        "{}",
        stderr(&output)
    );
    assert!(!p.root().join("bruno").exists());
    assert!(!stdout(&output).contains("script:"));
}

#[cfg(unix)]
mod symlinks {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn a_symlinked_bruno_directory_is_refused_and_the_target_is_untouched() {
        let p = golden_project();
        let outside = TestProject::new();
        symlink(outside.root(), p.root().join("bruno")).unwrap();
        let output = p.run(&["generate", "--force"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("symlink"), "{}", stderr(&output));
        assert!(tree(outside.root()).is_empty());
    }

    #[test]
    fn a_symlinked_controller_folder_is_refused_and_the_target_is_untouched() {
        let p = golden_project();
        let outside = TestProject::new();
        fs::create_dir(p.root().join("bruno")).unwrap();
        symlink(outside.root(), p.root().join("bruno/Users")).unwrap();
        let output = p.run(&["generate", "--force"]);
        assert_eq!(output.status.code(), Some(4));
        assert!(tree(outside.root()).is_empty());
        assert!(
            p.root().join("bruno/Auth/login.bru").is_file(),
            "other folders are still written"
        );
    }

    #[test]
    fn a_symlinked_request_file_is_never_written_through() {
        let p = golden_project();
        let outside = TestProject::new();
        let victim = outside.write("victim.txt", "precious");
        fs::create_dir_all(p.root().join("bruno/Users")).unwrap();
        symlink(&victim, p.root().join("bruno/Users/find-all.bru")).unwrap();
        for args in [&["generate"][..], &["generate", "--force"][..]] {
            let output = p.run(args);
            assert_eq!(output.status.code(), Some(4), "{args:?}");
            assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        }
    }
}

// ------------------------------------------------------------ scale

#[test]
fn five_hundred_routes_are_generated_with_unique_files() {
    let p = TestProject::new();
    for module in 0..5 {
        let mut source = format!("@Controller('m{module}') export class M{module}Controller {{\n");
        for i in 0..100 {
            source.push_str(&format!(
                "  @Get('r{i}/:id') handler{}(@Query('q{i}') q: string) {{}}\n",
                i % 40
            ));
        }
        source.push('}');
        p.write(format!("src/m{module}/m{module}.controller.ts"), &source);
    }
    let output = p.run(&["generate"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).starts_with("500 request(s): 500 created"),
        "{}",
        stdout(&output)
    );
    let files = tree(&p.root().join("bruno"));
    assert_eq!(files.len(), 501);
    let again = p.run(&["generate"]);
    assert!(stdout(&again).contains("500 unchanged"));
}
