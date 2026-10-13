//! Command implementations. Each command returns data or writes to `out`;
//! diagnostics go through the [`Reporter`].

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::bruno::generate;
use crate::bruno::writer::{FileStatus, WriteReport, write_collection};
use crate::cli::{Cli, Command};
use crate::config::{CONFIG_FILE_NAME, CONFIG_TEMPLATE};
use crate::diag::Reporter;
use crate::discovery::discover_controllers;
use crate::error::Error;
use crate::nest::analyze_files;
use crate::project::Project;
use crate::route::{Controller, ParameterName};
use crate::sync::{Desired, SyncResult, synchronize};
use crate::watch::{Notice, Watch, WatchOptions};
use crate::workspace::Delta;

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
    match &cli.command {
        Command::Init => init(cli, out).map(|()| Outcome::Success),
        Command::Check => check(cli, out, reporter).map(|()| Outcome::Success),
        Command::Discover => discover(cli, out, reporter),
        Command::Routes => routes(cli, out, reporter),
        Command::Generate { force } => generate_command(cli, *force, out, reporter),
        Command::Sync { dry_run } => sync_command(cli, *dry_run, out, reporter),
        Command::Watch { debounce } => watch_command(cli, *debounce, out, reporter),
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

/// Print every route, one block per route:
///
/// ```text
/// GET /users/:id  UsersController.findOne  src/users/users.controller.ts:12:3
///   @Param(id) #0
/// ```
///
/// Controllers are emitted in discovery order, routes in source order.
fn routes(cli: &Cli, out: &mut dyn Write, reporter: &Reporter) -> Result<Outcome, Error> {
    let project = open_project(cli, reporter)?;
    let discovery = discover_controllers(&project)?;
    let analysis = analyze_files(&project, &discovery.files)?;

    let diagnostics = discovery.diagnostics.iter().chain(&analysis.diagnostics);
    for diagnostic in diagnostics {
        reporter.report(diagnostic);
    }
    reporter.verbose(format_args!(
        "{} file(s) analyzed, {} controller(s)",
        discovery.files.len(),
        analysis.controllers.len()
    ));

    for controller in &analysis.controllers {
        write_routes(out, controller).map_err(Error::Output)?;
    }
    Ok(if discovery.is_incomplete() || analysis.is_incomplete() {
        Outcome::Incomplete
    } else {
        Outcome::Success
    })
}

fn write_routes(out: &mut dyn Write, controller: &Controller) -> std::io::Result<()> {
    for route in &controller.routes {
        writeln!(
            out,
            "{} {}  {}.{}  {}",
            route.http_method, route.path, controller.name, route.method_name, route.source
        )?;
        for parameter in &route.parameters {
            let name = match &parameter.name {
                ParameterName::Unnamed => String::new(),
                ParameterName::Named(name) => name.clone(),
                ParameterName::Unresolved => "<unresolved>".to_owned(),
            };
            writeln!(
                out,
                "  @{}({name}) #{}",
                parameter.kind.as_str(),
                parameter.index
            )?;
        }
    }
    Ok(())
}

/// discover ─▶ analyze ─▶ generate (pure) ─▶ write.
///
/// Incomplete discovery or analysis does not stop generation (this command
/// only ever creates or, with `--force`, replaces files, so a partial view
/// cannot cause anything to be deleted) but is reflected in the exit code.
fn generate_command(
    cli: &Cli,
    force: bool,
    out: &mut dyn Write,
    reporter: &Reporter,
) -> Result<Outcome, Error> {
    let project = open_project(cli, reporter)?;
    let discovery = discover_controllers(&project)?;
    let analysis = analyze_files(&project, &discovery.files)?;
    let generation = generate(project.config(), &analysis.controllers);
    // Nothing to write: do not create an empty collection directory.
    let report = if generation.requests.is_empty() {
        WriteReport::default()
    } else {
        write_collection(
            project.root(),
            &project.config().bruno,
            &generation.requests,
            force,
        )?
    };

    let diagnostics = discovery
        .diagnostics
        .iter()
        .chain(&analysis.diagnostics)
        .chain(&generation.diagnostics)
        .chain(&report.diagnostics);
    let mut generation_failed = false;
    for diagnostic in diagnostics {
        reporter.report(diagnostic);
    }
    for diagnostic in generation.diagnostics.iter().chain(&report.diagnostics) {
        generation_failed |= diagnostic.severity == crate::diag::Severity::Error;
    }

    for (path, status) in &report.files {
        let label = match status {
            FileStatus::Created => "created",
            FileStatus::Overwritten => "overwritten",
            FileStatus::Unchanged => "unchanged",
            FileStatus::SkippedExisting => "skipped",
            FileStatus::Failed => "failed",
        };
        reporter.verbose(format_args!("{label} {}", path.display()));
    }
    writeln!(
        out,
        "{} request(s): {} created, {} overwritten, {} unchanged, {} skipped (already exist), {} failed",
        generation.requests.len(),
        report.count(FileStatus::Created),
        report.count(FileStatus::Overwritten),
        report.count(FileStatus::Unchanged),
        report.count(FileStatus::SkippedExisting),
        report.count(FileStatus::Failed),
    )
    .map_err(Error::Output)?;

    let incomplete = discovery.is_incomplete()
        || analysis.is_incomplete()
        || generation_failed
        || report.has_failures();
    Ok(if incomplete {
        Outcome::Incomplete
    } else {
        Outcome::Success
    })
}

/// discover ─▶ analyze ─▶ generate (pure) ─▶ reconcile ─▶ apply (unless dry run).
fn sync_command(
    cli: &Cli,
    dry_run: bool,
    out: &mut dyn Write,
    reporter: &Reporter,
) -> Result<Outcome, Error> {
    let project = open_project(cli, reporter)?;
    let discovery = discover_controllers(&project)?;
    let analysis = analyze_files(&project, &discovery.files)?;
    let generation = generate(project.config(), &analysis.controllers);

    let source_complete = !discovery.is_incomplete() && !analysis.is_incomplete();
    let desired = Desired {
        requests: &generation.requests,
        skipped: &generation.skipped,
        complete: source_complete,
    };
    let result = synchronize(project.root(), &project.config().bruno, &desired, dry_run)?;

    let diagnostics = discovery
        .diagnostics
        .iter()
        .chain(&analysis.diagnostics)
        .chain(&generation.diagnostics)
        .chain(&result.diagnostics);
    for diagnostic in diagnostics {
        reporter.report(diagnostic);
    }

    write_sync_report(out, &project.config().bruno, &result, reporter.is_verbose())
        .map_err(Error::Output)?;

    let generation_failed = generation
        .diagnostics
        .iter()
        .any(|d| d.severity == crate::diag::Severity::Error);
    let incomplete = !source_complete || generation_failed || !result.is_complete();
    Ok(if incomplete {
        Outcome::Incomplete
    } else {
        Outcome::Success
    })
}

/// Deterministic, terminal-independent-enough report of a [`SyncResult`]: one
/// line per change or problem, then a summary.
fn write_sync_report(
    out: &mut dyn Write,
    bruno_dir: &std::path::Path,
    result: &SyncResult,
    verbose: bool,
) -> std::io::Result<()> {
    let show = |path: &std::path::Path| bruno_dir.join(path).display().to_string();
    for path in &result.created {
        writeln!(out, "CREATE    {}", show(path))?;
    }
    for path in &result.updated {
        writeln!(out, "UPDATE    {}", show(path))?;
    }
    for path in &result.deleted {
        writeln!(out, "DELETE    {}", show(path))?;
    }
    for conflict in &result.conflicts {
        writeln!(
            out,
            "CONFLICT  {}: {}",
            show(&conflict.path),
            conflict.reason.description()
        )?;
        writeln!(
            out,
            "          what to do: {}",
            conflict.reason.suggestion()
        )?;
    }
    for (path, reason) in &result.failed {
        writeln!(out, "FAILED    {}: {reason}", show(path))?;
    }
    if verbose {
        for path in &result.preserved {
            writeln!(out, "PRESERVE  {}", show(path))?;
        }
        for path in &result.unchanged {
            writeln!(out, "UNCHANGED {}", show(path))?;
        }
    }

    writeln!(out, "Created:     {}", result.created.len())?;
    writeln!(out, "Updated:     {}", result.updated.len())?;
    writeln!(out, "Unchanged:   {}", result.unchanged.len())?;
    writeln!(out, "Deleted:     {}", result.deleted.len())?;
    writeln!(out, "Preserved:   {}", result.preserved.len())?;
    writeln!(out, "Conflicts:   {}", result.conflicts.len())?;
    if !result.failed.is_empty() {
        writeln!(out, "Failed:      {}", result.failed.len())?;
    }
    if !result.applied {
        writeln!(out, "No files were written.")?;
    }
    Ok(())
}

/// Run watch mode until Ctrl-C (or SIGTERM).
fn watch_command(
    cli: &Cli,
    debounce_ms: u64,
    out: &mut dyn Write,
    reporter: &Reporter,
) -> Result<Outcome, Error> {
    let project = open_project(cli, reporter)?;
    project.ensure_source_dir()?;

    let watch = Watch::new();
    let for_signal = watch.stopper();
    ctrlc::set_handler(move || for_signal.stop())
        .map_err(|err| Error::Watch(format!("cannot install the signal handler: {err}")))?;

    // If the output cannot be written (a closed pipe), watching is pointless.
    let for_output = watch.stopper();
    let mut write_error: Option<std::io::Error> = None;
    let options = WatchOptions {
        debounce: std::time::Duration::from_millis(debounce_ms),
    };
    watch.run(project, &options, &mut |notice| {
        if write_error.is_none()
            && let Err(err) = present(out, reporter, &notice)
        {
            write_error = Some(err);
            for_output.stop();
        }
    })?;
    match write_error {
        Some(err) => Err(Error::Output(err)),
        None => Ok(Outcome::Success),
    }
}

/// Render one watch notice: results on stdout (flushed at once, since the
/// process is long-running), diagnostics on stderr.
fn present(out: &mut dyn Write, reporter: &Reporter, notice: &Notice) -> std::io::Result<()> {
    match notice {
        Notice::Initial(delta) => {
            report_diagnostics(reporter, delta);
            writeln!(out, "Initial sync:")?;
            write_counts(out, delta, true)?;
            write_problems(out, delta)?;
        }
        Notice::Ready {
            source,
            directories,
        } => {
            writeln!(
                out,
                "Watching {} ({directories} directories). Press Ctrl-C to stop.",
                source.display()
            )?;
        }
        Notice::Synced(delta) => {
            report_diagnostics(reporter, delta);
            for file in &delta.changed_files {
                writeln!(out, "Changed: {}", file.display())?;
            }
            writeln!(out, "  Routes affected: {}", delta.routes_affected)?;
            write_counts(out, delta, false)?;
            write_problems(out, delta)?;
        }
        Notice::Fallback { reason, delta } => {
            report_diagnostics(reporter, delta);
            writeln!(out, "Full resync ({reason}):")?;
            write_counts(out, delta, true)?;
            write_problems(out, delta)?;
        }
        Notice::ConfigReloaded => writeln!(out, "Configuration changed; reloaded.")?,
        Notice::Problem(diagnostic) => reporter.report(diagnostic),
        Notice::Stopped => writeln!(out, "Stopped.")?,
    }
    out.flush()
}

fn report_diagnostics(reporter: &Reporter, delta: &Delta) {
    for diagnostic in &delta.diagnostics {
        reporter.report(diagnostic);
    }
}

fn write_counts(out: &mut dyn Write, delta: &Delta, with_unchanged: bool) -> std::io::Result<()> {
    let result = &delta.result;
    write!(
        out,
        "  Created: {}  Updated: {}  Deleted: {}  Conflicts: {}",
        result.created.len(),
        result.updated.len(),
        result.deleted.len(),
        result.conflicts.len()
    )?;
    if with_unchanged {
        write!(out, "  Unchanged: {}", result.unchanged.len())?;
    }
    writeln!(out)
}

/// Conflicts and failed operations, which need the user's attention.
fn write_problems(out: &mut dyn Write, delta: &Delta) -> std::io::Result<()> {
    for conflict in &delta.result.conflicts {
        writeln!(
            out,
            "  CONFLICT {}: {}",
            conflict.path.display(),
            conflict.reason.description()
        )?;
        writeln!(out, "    what to do: {}", conflict.reason.suggestion())?;
    }
    for (path, reason) in &delta.result.failed {
        writeln!(out, "  FAILED {}: {reason}", path.display())?;
    }
    Ok(())
}
