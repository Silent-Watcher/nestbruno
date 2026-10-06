//! Integration tests for controller discovery: real temporary projects, run
//! both through the library API and through the actual binary.

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use common::{TestProject, discover, stderr, stdout};
use nestbruno::diag::Severity;
use nestbruno::discovery::{Discovery, MAX_DEPTH, discover_controllers};
use nestbruno::error::Error;
use nestbruno::project::Project;

fn paths(discovery: &Discovery) -> Vec<String> {
    discovery
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().replace('\\', "/"))
        .collect()
}

/// The layout from the phase specification.
fn reference_project() -> TestProject {
    let p = TestProject::new();
    p.write("src/users/users.controller.ts", "");
    p.write("src/users/users.service.ts", "");
    p.write("src/users/users.dto.ts", "");
    p.write("src/auth/auth.controller.ts", "");
    p.write("src/internal/helper.ts", "");
    p.write("node_modules/fake/fake.controller.ts", "");
    p.write("dist/users.controller.js", "");
    p.write(".git/HEAD", "ref: refs/heads/main\n");
    p
}

#[test]
fn reference_project_returns_only_controller_sources_sorted() {
    let p = reference_project();
    let discovery = discover(&p);
    assert_eq!(
        paths(&discovery),
        [
            "src/auth/auth.controller.ts",
            "src/users/users.controller.ts"
        ]
    );
    assert!(discovery.diagnostics.is_empty());
    assert!(!discovery.is_incomplete());
}

#[test]
fn empty_source_directory_yields_nothing() {
    let p = TestProject::new();
    fs::create_dir(p.root().join("src")).unwrap();
    let discovery = discover(&p);
    assert!(discovery.files.is_empty());
    assert_eq!(discovery.directories_read, 1);
}

#[test]
fn typescript_without_controller_candidates_yields_nothing() {
    let p = TestProject::new();
    p.write("src/a.service.ts", "");
    p.write("src/b/b.module.ts", "");
    assert!(discover(&p).files.is_empty());
}

#[test]
fn deeply_nested_controllers_are_found() {
    let p = TestProject::new();
    p.write("src/modules/admin/users/users.controller.ts", "");
    assert_eq!(
        paths(&discover(&p)),
        ["src/modules/admin/users/users.controller.ts"]
    );
}

#[test]
fn spaces_and_unicode_in_paths_are_handled() {
    let p = TestProject::new();
    p.write("src/my feature/users.controller.ts", "");
    p.write("src/کاربران/users.controller.ts", "");
    p.write("src/ünï-çødé/ünï.controller.ts", "");
    let found = paths(&discover(&p));
    assert_eq!(found.len(), 3);
    assert!(found.contains(&"src/my feature/users.controller.ts".to_owned()));
    assert!(found.contains(&"src/کاربران/users.controller.ts".to_owned()));
    assert!(found.contains(&"src/ünï-çødé/ünï.controller.ts".to_owned()));
}

#[cfg(unix)]
#[test]
fn non_utf8_file_names_are_handled() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let p = TestProject::new();
    fs::create_dir(p.root().join("src")).unwrap();
    let name = OsStr::from_bytes(b"bad-\xff-name.controller.ts");
    if fs::write(p.root().join("src").join(name), "").is_err() {
        return; // the filesystem refuses non-UTF-8 names; nothing to test
    }
    let discovery = discover(&p);
    assert_eq!(discovery.files.len(), 1);
    assert_eq!(
        discovery.files[0].path.file_name().unwrap().as_bytes(),
        b"bad-\xff-name.controller.ts"
    );
}

#[test]
fn nested_build_output_directories_are_excluded_by_default() {
    let p = TestProject::new();
    p.write("src/dist/users.controller.ts", "");
    p.write("src/build/users.controller.ts", "");
    p.write("src/coverage/a.controller.ts", "");
    p.write("src/.cache/a.controller.ts", "");
    p.write("src/.tmp/a.controller.ts", "");
    p.write("src/real/users.controller.ts", "");
    assert_eq!(paths(&discover(&p)), ["src/real/users.controller.ts"]);
}

#[test]
fn directories_are_pruned_not_just_filtered() {
    let p = TestProject::new();
    p.write("src/real/users.controller.ts", "");
    for i in 0..200 {
        p.write(format!("src/dist/deep/{i}/x.controller.ts"), "");
        p.write(format!("src/node_modules/pkg/{i}/x.controller.ts"), "");
    }
    // src and src/real are the only directories that may be listed.
    assert_eq!(discover(&p).directories_read, 2);
}

#[test]
fn node_modules_and_git_are_never_entered_even_with_empty_exclude() {
    let p = TestProject::new();
    p.write("nestbruno.toml", "source = \".\"\nexclude = []\n");
    p.write("node_modules/foo/test.controller.ts", "");
    p.write("src/node_modules/foo/test.controller.ts", "");
    p.write(".git/hooks/x.controller.ts", "");
    p.write("src/ok.controller.ts", "");
    assert_eq!(paths(&discover(&p)), ["src/ok.controller.ts"]);
}

#[test]
fn source_may_be_the_project_root_and_paths_have_no_dot_prefix() {
    let p = TestProject::new();
    p.write("nestbruno.toml", "source = \".\"\n");
    p.write("a.controller.ts", "");
    p.write("b/b.controller.ts", "");
    assert_eq!(
        paths(&discover(&p)),
        ["a.controller.ts", "b/b.controller.ts"]
    );
}

#[test]
fn a_source_directory_named_like_an_excluded_directory_still_works() {
    // Patterns are matched relative to the source root, so the legitimate
    // source directory `build` is not affected by the default `**/build/**`.
    let p = TestProject::new();
    p.write("nestbruno.toml", "source = \"build\"\n");
    p.write("build/users.controller.ts", "");
    p.write("build/build/nested.controller.ts", "");
    assert_eq!(paths(&discover(&p)), ["build/users.controller.ts"]);
}

#[test]
fn project_located_under_a_directory_named_dist_is_unaffected() {
    let p = TestProject::new();
    p.write("dist/project/src/users.controller.ts", "");
    let project = Project::open(&p.root().join("dist/project")).unwrap();
    let found = discover_controllers(&project).unwrap();
    assert_eq!(found.files.len(), 1);
}

#[test]
fn custom_include_finds_unconventionally_named_files() {
    let p = TestProject::new();
    p.write(
        "nestbruno.toml",
        "include = [\"**/*.controller.ts\", \"**/*-api.ts\"]\n",
    );
    p.write("src/custom-user-api.ts", "");
    p.write("src/users.controller.ts", "");
    p.write("src/users.ts", "");
    assert_eq!(
        paths(&discover(&p)),
        ["src/custom-user-api.ts", "src/users.controller.ts"]
    );
}

#[test]
fn exclude_takes_precedence_over_include() {
    let p = TestProject::new();
    p.write(
        "nestbruno.toml",
        "exclude = [\"**/generated/**\", \"**/legacy.controller.ts\"]\n",
    );
    p.write("src/generated/users.controller.ts", "");
    p.write("src/legacy.controller.ts", "");
    p.write("src/users.controller.ts", "");
    assert_eq!(paths(&discover(&p)), ["src/users.controller.ts"]);
}

#[test]
fn declaration_files_and_tsx_are_not_candidates() {
    let p = TestProject::new();
    p.write("nestbruno.toml", "include = [\"**/*\"]\n");
    p.write("src/a.d.ts", "");
    p.write("src/b.tsx", "");
    p.write("src/c.js", "");
    p.write("src/d.ts", "");
    assert_eq!(paths(&discover(&p)), ["src/d.ts"]);
}

#[test]
fn results_are_sorted_unique_and_repeatable() {
    let p = TestProject::new();
    // Created in anti-sorted order so traversal order cannot match sorted order.
    for dir in ["z", "m", "a", "a-b", "a b"] {
        for file in ["z", "a"] {
            p.write(format!("src/{dir}/{file}.controller.ts"), "");
        }
    }
    let first = discover(&p);
    let second = discover(&p);
    assert_eq!(first.files, second.files);

    let mut sorted = first.files.clone();
    sorted.sort();
    assert_eq!(first.files, sorted);

    let unique: HashSet<_> = first.files.iter().map(|f| &f.path).collect();
    assert_eq!(unique.len(), first.files.len());
    assert_eq!(first.files.len(), 10);
}

#[test]
fn excessive_nesting_is_reported_and_marks_the_result_incomplete() {
    let p = TestProject::new();
    p.write("src/shallow.controller.ts", "");
    let mut deep = PathBuf::from("src");
    for _ in 0..=MAX_DEPTH + 1 {
        deep.push("d");
    }
    p.write(deep.join("deep.controller.ts"), "");
    let discovery = discover(&p);
    assert_eq!(paths(&discovery), ["src/shallow.controller.ts"]);
    assert!(discovery.is_incomplete());
    assert!(discovery.diagnostics[0].message.contains("nesting"));
}

#[test]
fn large_project_is_discovered_correctly() {
    let p = TestProject::new();
    let mut expected = 0;
    for module in 0..100 {
        for file in 0..100 {
            let name = if file % 10 == 0 {
                expected += 1;
                format!("f{file}.controller.ts")
            } else {
                format!("f{file}.service.ts")
            };
            p.write(format!("src/m{module}/{name}"), "");
        }
    }
    for i in 0..300 {
        p.write(format!("node_modules/pkg{i}/index.controller.ts"), "");
    }
    let discovery = discover(&p);
    assert_eq!(discovery.files.len(), expected);
    assert_eq!(discovery.directories_read, 101); // src + 100 modules
}

// ---------------------------------------------------------------- errors

#[test]
fn missing_source_directory_is_a_fatal_error() {
    let p = TestProject::new();
    let project = Project::open(p.root()).unwrap();
    assert!(matches!(
        discover_controllers(&project).unwrap_err(),
        Error::SourceDirMissing { .. }
    ));
}

#[test]
fn source_that_is_a_file_is_a_fatal_error() {
    let p = TestProject::new();
    p.write("src", "not a directory");
    let project = Project::open(p.root()).unwrap();
    assert!(matches!(
        discover_controllers(&project).unwrap_err(),
        Error::SourceDirMissing { .. }
    ));
}

#[test]
fn cli_reports_invalid_patterns_without_panicking() {
    let p = TestProject::new();
    p.write("src/a.controller.ts", "");
    p.write("nestbruno.toml", "include = [\"[unclosed\"]\n");
    let output = p.run(&["discover"]);
    assert_eq!(output.status.code(), Some(3));
    let message = stderr(&output);
    assert!(message.contains("[unclosed"), "{message}");
    assert!(message.contains("include"), "{message}");
    assert!(!message.contains("panicked"), "{message}");

    p.write_over("nestbruno.toml", "exclude = [\"{a,b\"]\n");
    let output = p.run(&["discover"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains("exclude"));
}

// ---------------------------------------------------------------- symlinks

#[cfg(unix)]
mod symlinks {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn symlinks_inside_the_tree_are_not_followed() {
        let p = TestProject::new();
        p.write("src/real/users.controller.ts", "");
        p.write("outside/secret.controller.ts", "");
        symlink(p.root().join("outside"), p.root().join("src/link-dir")).unwrap();
        symlink(
            p.root().join("src/real/users.controller.ts"),
            p.root().join("src/link.controller.ts"),
        )
        .unwrap();
        let discovery = discover(&p);
        assert_eq!(paths(&discovery), ["src/real/users.controller.ts"]);
        let notes: Vec<_> = discovery
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Note)
            .collect();
        assert_eq!(notes.len(), 2);
        assert!(!discovery.is_incomplete());
    }

    #[test]
    fn symlink_loops_terminate() {
        let p = TestProject::new();
        p.write("src/a.controller.ts", "");
        symlink(p.root().join("src"), p.root().join("src/loop")).unwrap();
        assert_eq!(paths(&discover(&p)), ["src/a.controller.ts"]);
    }

    #[test]
    fn source_symlinked_outside_the_project_is_rejected() {
        let project = TestProject::new();
        let outside = TestProject::new();
        outside.write("evil.controller.ts", "");
        symlink(outside.root(), project.root().join("src")).unwrap();
        let opened = Project::open(project.root()).unwrap();
        let err = discover_controllers(&opened).unwrap_err();
        assert!(matches!(err, Error::SourceOutsideProject { .. }), "{err}");
        assert!(err.to_string().contains("outside the project"));
    }

    #[test]
    fn source_symlinked_to_another_directory_inside_the_project_is_allowed() {
        let p = TestProject::new();
        p.write("packages/api/users.controller.ts", "");
        symlink(p.root().join("packages/api"), p.root().join("src")).unwrap();
        assert_eq!(paths(&discover(&p)), ["src/users.controller.ts"]);
    }

    // --------------------------------------------------------- permissions

    use std::os::unix::fs::PermissionsExt;

    /// Restores permissions so the temp dir can be deleted even if an assert fails.
    struct Unlock(PathBuf);
    impl Drop for Unlock {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn unreadable_subdirectory_is_recoverable_and_marks_result_incomplete() {
        let p = TestProject::new();
        p.write("src/ok/ok.controller.ts", "");
        p.write("src/locked/hidden.controller.ts", "");
        let locked = p.root().join("src/locked");
        let _unlock = Unlock(locked.clone());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&locked).is_ok() {
            // Running as root (or on a filesystem ignoring modes): the
            // directory is readable, so there is nothing to observe.
            return;
        }

        let discovery = discover(&p);
        assert_eq!(paths(&discovery), ["src/ok/ok.controller.ts"]);
        assert!(discovery.is_incomplete());
        let diagnostic = &discovery.diagnostics[0];
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(
            diagnostic.path.as_deref(),
            Some(std::path::Path::new("src/locked"))
        );

        let output = p.run(&["discover"]);
        assert_eq!(output.status.code(), Some(4));
        assert_eq!(stdout(&output), "src/ok/ok.controller.ts\n");
        assert!(stderr(&output).contains("src/locked"));
    }

    #[test]
    fn unreadable_source_root_is_fatal() {
        let p = TestProject::new();
        p.write("src/a.controller.ts", "");
        let src = p.root().join("src");
        let _unlock = Unlock(src.clone());
        fs::set_permissions(&src, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&src).is_ok() {
            return; // see above
        }
        let project = Project::open(p.root()).unwrap();
        assert!(matches!(
            discover_controllers(&project).unwrap_err(),
            Error::Io { .. }
        ));
    }
}

// ---------------------------------------------------------------- CLI

#[test]
fn cli_prints_one_path_per_line_in_sorted_order() {
    let p = reference_project();
    let output = p.run(&["discover"]);
    assert!(output.status.success(), "{}", stderr(&output));
    // Normalize separators so the assertion is platform independent.
    let text = stdout(&output).replace('\\', "/");
    assert_eq!(
        text,
        "src/auth/auth.controller.ts\nsrc/users/users.controller.ts\n"
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn cli_with_no_controllers_prints_nothing_and_succeeds() {
    let p = TestProject::new();
    p.write("src/a.service.ts", "");
    let output = p.run(&["discover"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
}

#[test]
fn cli_verbose_shows_notes_and_statistics_on_stderr() {
    let p = reference_project();
    let output = p.run(&["discover", "--verbose"]);
    assert!(
        stderr(&output).contains("2 candidate file(s)"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn cli_missing_source_directory_exits_with_config_code() {
    let p = TestProject::new();
    let output = p.run(&["discover"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains("does not exist"));
}
