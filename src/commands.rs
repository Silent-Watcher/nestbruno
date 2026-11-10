//! Command implementations. Each command returns data or writes to `out`;
//! diagnostics go through the [`Reporter`].

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::bruno::generate;
use crate::bruno::writer::{EnvironmentReport, FileStatus, WriteReport, write_collection};
use crate::cli::{Cli, Command, OutputFormat};
use crate::config::{CONFIG_FILE_NAME, CONFIG_TEMPLATE};
use crate::diag::Reporter;
use crate::discovery::discover_controllers;
use crate::error::Error;
use crate::nest::analyze_files;
use crate::output;
use crate::project::Project;
use crate::route::{Controller, ParameterName};
use crate::sync::{Desired, SyncResult, synchronize};
use crate::watch::{Notice, Watch, WatchOptions};
use crate::workspace::Delta;

/// How a command that did not fail fatally ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// Finished with a complete state, but the collection is out of sync
    /// (`verify`): suitable as a CI failure signal.
    OutOfSync,
    /// Finished, but some paths could not be processed; the result is
    /// incomplete (maps to a distinct exit code).
    Incomplete,
}

/// Run the command selected on the command line.
pub fn execute(cli: &Cli, out: &mut dyn Write, reporter: &Reporter) -> Result<Outcome, Error> {
    match &cli.command {
        Command::Init => init(cli, out).map(|()| Outcome::Success),
        Command::Check => check(cli, out, reporter).map(|()| Outcome::Success),
        Command::Discover { format } => discover(cli, *format, out, reporter),
        Command::Routes { format } => routes(cli, *format, out, reporter),
        Command::Generate {
            force,
            environments,
        } => generate_command(cli, *force, environments.clone(), out, reporter),
        Command::Sync {
            dry_run,
            staged: staged_only,
            format,
        } => sync_command(cli, *dry_run, *staged_only, *format, out, reporter),
        Command::Verify {
            staged: staged_only,
            format,
        } => verify_command(cli, *staged_only, *format, out, reporter),
        Command::Explain {
            selector,
            controller,
            handler,
            file,
            format,
        } => explain_command(
            cli,
            ExplainQuery {
                selector: selector.as_ref(),
                controller: controller.as_deref(),
                handler: handler.as_deref(),
                file: file.as_deref(),
            },
            *format,
            out,
            reporter,
        ),
        Command::Completions { shell } => {
            let mut command = <Cli as clap::CommandFactory>::command();
            clap_complete::generate(*shell, &mut command, "nestbruno", out);
            Ok(Outcome::Success)
        }
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

fn discover(
    cli: &Cli,
    format: OutputFormat,
    out: &mut dyn Write,
    reporter: &Reporter,
) -> Result<Outcome, Error> {
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

    match format {
        OutputFormat::Text => {
            for file in &discovery.files {
                writeln!(out, "{}", file.path.display()).map_err(Error::Output)?;
            }
        }
        OutputFormat::Json => output::write_json(out, &output::Discover::from(&discovery))?,
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
fn routes(
    cli: &Cli,
    format: OutputFormat,
    out: &mut dyn Write,
    reporter: &Reporter,
) -> Result<Outcome, Error> {
    let project = open_project(cli, reporter)?;
    let discovery = discover_controllers(&project)?;
    let analysis = analyze_files(&project, &discovery.files)?;

    let diagnostics: Vec<_> = discovery
        .diagnostics
        .iter()
        .chain(&analysis.diagnostics)
        .cloned()
        .collect();
    for diagnostic in &diagnostics {
        reporter.report(diagnostic);
    }
    reporter.verbose(format_args!(
        "{} file(s) analyzed, {} controller(s)",
        discovery.files.len(),
        analysis.controllers.len()
    ));

    let complete = !discovery.is_incomplete() && !analysis.is_incomplete();
    match format {
        OutputFormat::Text => {
            for controller in &analysis.controllers {
                write_routes(out, controller).map_err(Error::Output)?;
            }
        }
        OutputFormat::Json => {
            output::write_json(
                out,
                &output::Routes::from_analysis(&analysis.controllers, &diagnostics, complete),
            )?;
        }
    }
    Ok(if complete {
        Outcome::Success
    } else {
        Outcome::Incomplete
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
    environments: Option<Vec<String>>,
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

    // Environment files (Phase 16): created only when absent, with empty
    // placeholders. Values are never invented, and an existing environment
    // file is user-owned — reported, never modified.
    let environment_report = environments
        .filter(|_| !generation.requests.is_empty())
        .map(
            |names| -> Result<Option<(Vec<String>, usize, EnvironmentReport)>, Error> {
                let registry =
                    crate::bruno::environment::collect_environment_variables(&generation.requests);
                for diagnostic in &registry.diagnostics {
                    reporter.report(diagnostic);
                }
                if registry.variables.is_empty() {
                    reporter.verbose(format_args!(
                        "no environment variables referenced; no environment files written"
                    ));
                    return Ok(None);
                }
                let mut names: Vec<String> = names
                    .into_iter()
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty())
                    .collect();
                names.sort();
                names.dedup();
                if names.is_empty() {
                    return Err(Error::Usage {
                        message: "no valid environment names given".to_owned(),
                    });
                }
                let content = crate::bruno::environment::environment_content(&registry.variables)
                    .map_err(|reason| Error::UnsafeOutput {
                    path: project.config().bruno.clone(),
                    reason,
                })?;
                let report = crate::bruno::writer::write_environments(
                    project.root(),
                    &project.config().bruno,
                    &names,
                    &content,
                )?;
                Ok(Some((names, registry.variables.len(), report)))
            },
        )
        .transpose()?
        .flatten();

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

    // Environment summary, only when the flag was used.
    let mut environments_failed = false;
    if let Some((names, variables, env_report)) = &environment_report {
        for (path, reason) in &env_report.failed {
            reporter.report(&crate::diag::Diagnostic::new(
                crate::diag::Severity::Error,
                Some(path.clone()),
                format!("environment file not written: {reason}"),
            ));
        }
        environments_failed = env_report.has_failures();
        let mut parts = Vec::new();
        if !env_report.created.is_empty() {
            parts.push(format!("{} created", env_report.created.len()));
        }
        if !env_report.existing.is_empty() {
            parts.push(format!(
                "{} already exist(s) (not modified)",
                env_report.existing.len()
            ));
        }
        if env_report.failed.is_empty() && parts.is_empty() {
            parts.push("none written".to_owned());
        }
        writeln!(
            out,
            "Environments ({}): {} — {} variable(s): {}",
            names.join(", "),
            parts.join(", "),
            variables,
            if env_report.failed.is_empty() {
                "placeholders are empty; fill in your values"
            } else {
                "some files were not written (see the error diagnostics)"
            }
        )
        .map_err(Error::Output)?;
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
        || report.has_failures()
        || environments_failed;
    Ok(if incomplete {
        Outcome::Incomplete
    } else {
        Outcome::Success
    })
}

/// The analysis and planning part of synchronization, shared by `sync` and
/// `verify` (Phase 14.2): analyze the working tree or the git index, generate
/// the desired requests, reconcile with the collection. With `dry_run` nothing
/// is ever written.
struct SyncRun {
    /// Plan result with **all** diagnostics merged in report order (source
    /// analysis, generation, synchronization).
    result: SyncResult,
    /// Staged changes when the run was staged-only (empty otherwise).
    staged_changes: Vec<crate::staged::StagedChange>,
    /// Whether the desired state could be established completely
    /// (discovery, analysis and generation covered everything).
    analysis_complete: bool,
    /// Staged-only with nothing relevant staged: a clean no-op.
    no_staged_changes: bool,
}

fn analyze_and_plan(project: &Project, staged_only: bool, dry_run: bool) -> Result<SyncRun, Error> {
    let (controllers, mut diagnostics, source_complete, staged_changes) = if staged_only {
        let matcher = crate::discovery::Matcher::new(project.config())?;
        let staged = crate::staged::staged_sources(project, &matcher)?;
        if staged.is_empty() {
            return Ok(SyncRun {
                result: SyncResult::default(),
                staged_changes: Vec::new(),
                analysis_complete: true,
                no_staged_changes: true,
            });
        }
        let analysis = crate::staged::analyze_staged(project, &staged)?;
        let source_complete = !staged
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == crate::diag::Severity::Error)
            && !analysis.is_incomplete();
        let mut diagnostics = staged.diagnostics;
        diagnostics.extend(analysis.diagnostics);
        (
            analysis.controllers,
            diagnostics,
            source_complete,
            staged.changes,
        )
    } else {
        let discovery = discover_controllers(project)?;
        let analysis = analyze_files(project, &discovery.files)?;
        let source_complete = !discovery.is_incomplete() && !analysis.is_incomplete();
        let mut diagnostics = discovery.diagnostics;
        diagnostics.extend(analysis.diagnostics);
        (
            analysis.controllers,
            diagnostics,
            source_complete,
            Vec::new(),
        )
    };

    let generation = generate(project.config(), &controllers);
    let generation_failed = generation
        .diagnostics
        .iter()
        .any(|d| d.severity == crate::diag::Severity::Error);
    let desired = Desired {
        requests: &generation.requests,
        skipped: &generation.skipped,
        complete: source_complete,
    };
    let mut result = synchronize(project.root(), &project.config().bruno, &desired, dry_run)?;
    // One merged, deterministically ordered list: source diagnostics, then
    // generation diagnostics, then the synchronization result's own.
    diagnostics.extend(generation.diagnostics);
    diagnostics.append(&mut result.diagnostics);
    result.diagnostics = diagnostics;
    Ok(SyncRun {
        result,
        staged_changes,
        analysis_complete: source_complete && !generation_failed,
        no_staged_changes: false,
    })
}

/// discover ─▶ analyze ─▶ generate (pure) ─▶ reconcile ─▶ apply (unless dry run).
///
/// With `staged_only`, the state comes from the git index instead of the
/// working tree: the desired routes are reconstructed from the staged content
/// of every relevant file, and the run is a no-op unless the staging area
/// itself touches a relevant source file.
fn sync_command(
    cli: &Cli,
    dry_run: bool,
    staged_only: bool,
    format: OutputFormat,
    out: &mut dyn Write,
    reporter: &Reporter,
) -> Result<Outcome, Error> {
    let project = open_project(cli, reporter)?;
    let run = analyze_and_plan(&project, staged_only, dry_run)?;

    if run.no_staged_changes {
        if format == OutputFormat::Json {
            // One document even when there is nothing to do.
            output::write_json(
                out,
                &output::Sync {
                    dry_run,
                    complete: true,
                    ..output::Sync::default()
                },
            )?;
        } else {
            writeln!(out, "No relevant staged NestJS changes detected.").map_err(Error::Output)?;
        }
        return Ok(Outcome::Success);
    }
    if staged_only && format == OutputFormat::Text {
        writeln!(
            out,
            "Staged: {} relevant change(s):",
            run.staged_changes.len()
        )
        .map_err(Error::Output)?;
        for change in &run.staged_changes {
            writeln!(
                out,
                "  {} {}",
                change.status.as_str(),
                change.path.display()
            )
            .map_err(Error::Output)?;
        }
    }

    for diagnostic in &run.result.diagnostics {
        reporter.report(diagnostic);
    }

    let complete = run.analysis_complete && run.result.is_complete();
    match format {
        OutputFormat::Text => write_sync_report(
            out,
            &project.config().bruno,
            &run.result,
            reporter.is_verbose(),
        )
        .map_err(Error::Output)?,
        OutputFormat::Json => output::write_json(
            out,
            &output::Sync::from_result(
                &project.config().bruno,
                &run.result,
                dry_run,
                complete,
                &run.staged_changes,
            ),
        )?,
    }
    Ok(if complete {
        Outcome::Success
    } else {
        Outcome::Incomplete
    })
}

/// verify ─▶ shared pipeline as a dry run ─▶ report. Strictly read-only.
fn verify_command(
    cli: &Cli,
    staged_only: bool,
    format: OutputFormat,
    out: &mut dyn Write,
    reporter: &Reporter,
) -> Result<Outcome, Error> {
    let project = open_project(cli, reporter)?;
    // `dry_run = true`: the pipeline plans but never applies; verify has no
    // other execution path, so it cannot write anything.
    let run = analyze_and_plan(&project, staged_only, true)?;

    if run.no_staged_changes {
        if format == OutputFormat::Json {
            output::write_json(
                out,
                &output::Verify {
                    status: "no_staged_changes",
                    complete: true,
                    ..output::Verify::default()
                },
            )?;
        } else {
            writeln!(out, "No relevant staged NestJS changes; nothing to verify.")
                .map_err(Error::Output)?;
        }
        return Ok(Outcome::Success);
    }

    for diagnostic in &run.result.diagnostics {
        reporter.report(diagnostic);
    }

    let result = &run.result;
    let source_errors = result
        .diagnostics
        .iter()
        .any(|d| d.severity == crate::diag::Severity::Error);
    let status = if !run.analysis_complete {
        "incomplete"
    } else if !result.conflicts.is_empty() || !result.failed.is_empty() {
        "blocked"
    } else if source_errors {
        "incomplete"
    } else if result.has_changes() {
        "out_of_sync"
    } else {
        "in_sync"
    };
    let complete = run.analysis_complete && result.is_complete();
    match format {
        OutputFormat::Text => write_verify_report(out, &project.config().bruno, result, status)
            .map_err(Error::Output)?,
        OutputFormat::Json => output::write_json(
            out,
            &output::Verify::from_result(
                &project.config().bruno,
                result,
                &run.staged_changes,
                status,
                complete,
            ),
        )?,
    }
    Ok(match status {
        "in_sync" => Outcome::Success,
        "out_of_sync" => Outcome::OutOfSync,
        // Conflicts and an incomplete analysis mean the result cannot be
        // acted on automatically: the existing "incomplete" exit code.
        _ => Outcome::Incomplete,
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

/// Read-only verification report: status, the category counts, the details
/// that need attention, and an honest footer (nothing was modified).
fn write_verify_report(
    out: &mut dyn Write,
    bruno_dir: &std::path::Path,
    result: &SyncResult,
    status: &'static str,
) -> std::io::Result<()> {
    let show = |path: &std::path::Path| bruno_dir.join(path).display().to_string();
    let label = match status {
        "in_sync" => "IN SYNC",
        "out_of_sync" => "OUT OF SYNC",
        "blocked" => "BLOCKED",
        "incomplete" => "INCOMPLETE",
        _ => "NO STAGED CHANGES",
    };
    writeln!(out, "NestBruno Verification")?;
    writeln!(out)?;
    writeln!(out, "Status: {label}")?;
    writeln!(out)?;
    writeln!(out, "Missing:   {}", result.created.len())?;
    writeln!(out, "Outdated:  {}", result.updated.len())?;
    writeln!(out, "Stale:     {}", result.deleted.len())?;
    writeln!(out, "Unchanged: {}", result.unchanged.len())?;
    writeln!(out, "Conflicts: {}", result.conflicts.len())?;
    writeln!(out, "Preserved: {}", result.preserved.len())?;
    if !result.failed.is_empty() {
        writeln!(out, "Failed:    {}", result.failed.len())?;
    }
    for conflict in &result.conflicts {
        writeln!(out)?;
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
        writeln!(out)?;
        writeln!(out, "FAILED    {}: {reason}", show(path))?;
    }
    writeln!(out)?;
    match status {
        "in_sync" => writeln!(out, "Verification passed. The collection is synchronized.")?,
        "out_of_sync" => {
            writeln!(out, "Verification failed.")?;
            writeln!(out, "Run `nestbruno sync` to reconcile the collection.")?;
        }
        "blocked" => {
            writeln!(out, "Verification failed.")?;
            writeln!(
                out,
                "Resolve the conflicts, then run `nestbruno sync` again."
            )?;
        }
        "incomplete" => {
            writeln!(
                out,
                "Verification could not establish a complete state; see the error: diagnostics on stderr."
            )?;
            writeln!(out, "Fix the problems, then run `nestbruno verify` again.")?;
        }
        _ => writeln!(out, "No relevant staged NestJS changes; nothing to verify.")?,
    }
    writeln!(out, "The collection was not modified.")
}

/// What `explain` was asked to look for (the CLI selector inputs).
struct ExplainQuery<'a> {
    selector: Option<&'a crate::explain::RouteSelector>,
    controller: Option<&'a str>,
    handler: Option<&'a str>,
    file: Option<&'a std::path::Path>,
}

/// explain ─▶ (the same analysis pipeline as `routes`) ─▶ select ─▶ report.
///
/// Strictly read-only: it explains the analyzer's findings, it never touches
/// the collection or the sources.
fn explain_command(
    cli: &Cli,
    query: ExplainQuery<'_>,
    format: OutputFormat,
    out: &mut dyn Write,
    reporter: &Reporter,
) -> Result<Outcome, Error> {
    let ExplainQuery {
        selector,
        controller,
        handler,
        file,
    } = query;
    // clap already enforces "selector or handler"; the explicit guard keeps
    // the contract independent of clap configuration.
    let selector = selector.cloned().or_else(|| {
        handler.map(|handler| crate::explain::RouteSelector::Handler {
            controller: controller.map(str::to_owned),
            handler: handler.to_owned(),
        })
    });
    let Some(selector) = selector else {
        return Err(Error::Usage {
            message: "explain needs a route selector: \"METHOD /path\", or --handler (optionally with --controller)"
                .to_owned(),
        });
    };

    let project = open_project(cli, reporter)?;
    let discovery = discover_controllers(&project)?;
    let analysis = analyze_files(&project, &discovery.files)?;
    let complete = !discovery.is_incomplete() && !analysis.is_incomplete();

    for diagnostic in discovery.diagnostics.iter().chain(&analysis.diagnostics) {
        reporter.report(diagnostic);
    }

    let matches = crate::explain::find_matches(&analysis.controllers, &selector, file);
    match matches.as_slice() {
        [(controller, route)] => {
            let bootstrap = crate::nest::bootstrap_for(&project);
            let explanation = crate::explain::build_explanation(
                controller,
                route,
                &bootstrap,
                &analysis.diagnostics,
                complete,
            );
            match format {
                OutputFormat::Text => {
                    write_explain_report(out, &explanation).map_err(Error::Output)?
                }
                OutputFormat::Json => {
                    output::write_json(out, &output::Explain::from_explanation(&explanation))?
                }
            }
        }
        [] => {
            if format == OutputFormat::Json {
                output::write_json(out, &output::Explain::unmatched(Vec::new(), complete))?;
            }
            return Err(Error::RouteNotFound {
                selector: selector.to_string(),
            });
        }
        many => {
            let entries: Vec<output::RouteEntry> = many
                .iter()
                .map(|(controller, route)| output::RouteEntry::of(&controller.name, route))
                .collect();
            if format == OutputFormat::Json {
                output::write_json(out, &output::Explain::unmatched(entries.clone(), complete))?;
            }
            let list = many
                .iter()
                .map(|(controller, route)| {
                    format!(
                        "{} {} ({}.{} at {})",
                        route.http_method,
                        route.path,
                        controller.name,
                        route.method_name,
                        route.source
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Error::AmbiguousRoutes {
                selector: selector.to_string(),
                count: many.len(),
                list,
            });
        }
    }
    Ok(if complete {
        Outcome::Success
    } else {
        Outcome::Incomplete
    })
}

/// Human-readable explanation: identification, derived resolution, the
/// metadata the analyzer holds, the findings checklist and the diagnostics.
fn write_explain_report(
    out: &mut dyn Write,
    explanation: &crate::explain::Explanation,
) -> std::io::Result<()> {
    use crate::route::{BodyShape, RoutePath};

    let route = &explanation.route;
    let resolution = &explanation.resolution;
    writeln!(out, "Route Explanation")?;
    writeln!(out)?;
    writeln!(
        out,
        "Route:      {} {}",
        route.http_method.as_str(),
        route.path
    )?;
    writeln!(out, "Controller: {}", explanation.controller_name)?;
    writeln!(out, "Handler:    {}", route.method_name)?;
    writeln!(out, "Source:     {}", route.source)?;
    writeln!(out)?;
    writeln!(out, "Resolution:")?;
    let labeled = |label: &str, value: Option<&str>| match value {
        Some(value) => format!("  {label:<18} {value}"),
        None => format!("  {label:<18} <unknown>"),
    };
    writeln!(
        out,
        "{}",
        labeled(
            "Controller prefix:",
            resolution.controller_prefix.as_deref(),
        )
    )?;
    writeln!(
        out,
        "{}",
        match &resolution.global_prefix {
            Some(prefix) => format!(
                "  {:<18} {prefix} ({})",
                "Global prefix:",
                if resolution.prefix_applied {
                    "applied"
                } else {
                    "not applied"
                }
            ),
            None => format!(
                "  {:<18} none (no statically provable setGlobalPrefix)",
                "Global prefix:"
            ),
        }
    )?;
    match &resolution.version_segment {
        Some(version) => writeln!(out, "  {:<18} {version}", "Version segment:")?,
        None => writeln!(out, "  {:<18} none", "Version segment:")?,
    }
    writeln!(
        out,
        "{}",
        labeled("Handler path:", resolution.handler_path.as_deref())
    )?;
    match &route.path {
        RoutePath::Static(path) => writeln!(out, "  {:<18} {path}", "Final route:")?,
        RoutePath::Unresolved => writeln!(out, "  {:<18} <unresolved>", "Final route:")?,
    }
    for limitation in &explanation.limitations {
        writeln!(out, "  Note: {limitation}")?;
    }
    writeln!(out)?;
    writeln!(out, "Metadata:")?;
    if route.parameters.is_empty() {
        writeln!(out, "  Parameters: none")?;
    } else {
        writeln!(out, "  Parameters:")?;
        for parameter in &route.parameters {
            let name = match &parameter.name {
                ParameterName::Unnamed => "()".to_owned(),
                ParameterName::Named(name) => format!("({name})"),
                ParameterName::Unresolved => "(<unresolved>)".to_owned(),
            };
            let mut line = format!(
                "    - @{}{name} #{}",
                parameter.kind.as_str(),
                parameter.index
            );
            if let Some(type_name) = &parameter.type_name {
                line.push_str(&format!(" ({type_name})"));
            }
            if !parameter.fields.is_empty() {
                line.push_str(&format!(" [fields: {}]", parameter.fields.join(", ")));
            }
            writeln!(out, "{line}")?;
        }
    }
    writeln!(
        out,
        "  Body: {}",
        match &route.body {
            BodyShape::None => "none".to_owned(),
            BodyShape::Unknown => "unknown (could not be resolved statically)".to_owned(),
            BodyShape::Fields(fields) => format!("fields: {}", fields.join(", ")),
        }
    )?;
    writeln!(out)?;
    writeln!(out, "Analysis:")?;
    for step in &explanation.trace {
        match &step.detail {
            Some(detail) => writeln!(out, "  - {}: {detail}", step.finding)?,
            None => writeln!(out, "  - {}", step.finding)?,
        }
    }
    writeln!(out)?;
    writeln!(out, "Warnings:")?;
    if explanation.warnings.is_empty() {
        writeln!(out, "  None")?;
    } else {
        for diagnostic in &explanation.warnings {
            writeln!(out, "  {diagnostic}")?;
        }
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
