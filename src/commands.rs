//! Command implementations. Each command returns data or writes to `out`;
//! diagnostics go through the [`Reporter`].

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::cli::{Cli, Command};
use crate::config::{CONFIG_FILE_NAME, CONFIG_TEMPLATE};
use crate::diag::Reporter;
use crate::discovery::discover_controllers;
use crate::error::Error;
use crate::project::Project;

/// How a command that did not fail fatally ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// Finished, but some paths could not be processed; the result is
    /// incomplete (maps to a distinct exit code).
    Incomplete,
}

/// Run the command selected on the command line.
pub fn execute(cli: &Cli, out: &mut dyn Write, reporter: &Reporter) -> Result<Outcome, Error> {
    match cli.command {
        Command::Init => init(cli, out).map(|()| Outcome::Success),
        Command::Check => check(cli, out, reporter).map(|()| Outcome::Success),
        Command::Discover => discover(cli, out, reporter),
        Command::Sync => {
            open_project(cli, reporter)?;
            Err(Error::NotImplemented {
                command: "sync",
                phase: 5,
            })
        }
        Command::Watch => {
            open_project(cli, reporter)?;
            Err(Error::NotImplemented {
                command: "watch",
                phase: 7,
            })
        }
    }
}

/// Resolve the project root and load its configuration.
///
/// An explicit `--project` is used as given; otherwise the root is found by
/// walking up from the current directory.
fn open_project(cli: &Cli, reporter: &Reporter) -> Result<Project, Error> {
    let root = match &cli.project {
        Some(dir) => dir.clone(),
        None => {
            let cwd = current_dir()?;
            Project::find_root(&cwd)?
        }
    };
    let project = Project::open(&root)?;
    match project.config_path() {
        Some(path) => reporter.verbose(format_args!("using configuration {}", path.display())),
        None => reporter.verbose(format_args!(
            "no {CONFIG_FILE_NAME} found in {}; using defaults",
            project.root().display()
        )),
    }
    Ok(project)
}

fn current_dir() -> Result<PathBuf, Error> {
    std::env::current_dir().map_err(|source| Error::Io {
        path: PathBuf::from("."),
        source,
    })
}

fn init(cli: &Cli, out: &mut dyn Write) -> Result<(), Error> {
    let dir = match &cli.project {
        Some(dir) => dir.clone(),
        None => current_dir()?,
    };
    let path = dir.join(CONFIG_FILE_NAME);
    // `create_new` makes the existence check and the creation a single atomic
    // step, so an existing file can never be overwritten.
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Error::AlreadyExists { path });
        }
        Err(source) => return Err(Error::Io { path, source }),
    };
    file.write_all(CONFIG_TEMPLATE.as_bytes())
        .map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
    writeln!(out, "created {}", path.display()).map_err(Error::Output)
}

fn check(cli: &Cli, out: &mut dyn Write, reporter: &Reporter) -> Result<(), Error> {
    let project = open_project(cli, reporter)?;
    project.ensure_source_dir()?;
    let config = project.config();
    let config_file = project
        .config_path()
        .map_or_else(|| "(defaults)".to_owned(), |p| p.display().to_string());

    let result = (|| {
        writeln!(out, "project: {}", project.root().display())?;
        writeln!(out, "config:  {config_file}")?;
        writeln!(out, "source:  {}", config.source.display())?;
        writeln!(out, "bruno:   {}", config.bruno.display())?;
        writeln!(out, "include: {}", config.include.join(", "))?;
        writeln!(out, "exclude: {}", config.exclude.join(", "))?;
        writeln!(out, "ok")
    })();
    result.map_err(Error::Output)
}

fn discover(cli: &Cli, out: &mut dyn Write, reporter: &Reporter) -> Result<Outcome, Error> {
    let project = open_project(cli, reporter)?;
    let discovery = discover_controllers(&project)?;

    for diagnostic in &discovery.diagnostics {
        reporter.report(diagnostic);
    }
    reporter.verbose(format_args!(
        "{} candidate file(s), {} directories read",
        discovery.files.len(),
        discovery.directories_read
    ));

    for file in &discovery.files {
        writeln!(out, "{}", file.path.display()).map_err(Error::Output)?;
    }
    Ok(if discovery.is_incomplete() {
        Outcome::Incomplete
    } else {
        Outcome::Success
    })
}
