//! Machine-readable JSON output for the commands that produce structured
//! results (`--format json`, Phase 14.1).
//!
//! This is a presentation layer, not a new model: every document is built
//! from the existing result types ([`crate::route`], [`crate::discovery`],
//! [`crate::sync`]) without duplicating any logic. Exactly one complete JSON
//! document is written to stdout per invocation; diagnostics are represented
//! as structured objects and never corrupt the document.
//!
//! Field and variant names are part of the output contract and change only
//! with the project's versioning. Ordering is deterministic: fields follow
//! the struct definitions, lists follow the deterministic order of the
//! underlying results.

use std::io::Write;

use serde::Serialize;

use crate::diag::{Diagnostic, Severity};
use crate::discovery::Discovery;
use crate::error::Error;
use crate::explain::{Explanation, Resolution};
use crate::route::{BodyShape, Controller, ParameterKind, ParameterName, Route, RoutePath};
use crate::staged::{StagedChange, StagedStatus};
use crate::sync::SyncResult;

/// Serialize `doc` as the single JSON document written to stdout.
pub fn write_json(out: &mut dyn Write, doc: &impl Serialize) -> Result<(), Error> {
    serde_json::to_writer_pretty(&mut *out, doc).map_err(Error::Json)?;
    writeln!(out).map_err(Error::Output)
}

/// `routes --format json`
#[derive(Serialize)]
pub struct Routes {
    pub routes: Vec<RouteEntry>,
    pub diagnostics: Vec<JsonDiagnostic>,
    pub complete: bool,
}

impl Routes {
    pub fn from_analysis(
        controllers: &[Controller],
        diagnostics: &[Diagnostic],
        complete: bool,
    ) -> Self {
        Self {
            routes: controllers
                .iter()
                .flat_map(|c| c.routes.iter().map(|r| RouteEntry::new(c, r)))
                .collect(),
            diagnostics: diagnostics_from(diagnostics),
            complete,
        }
    }
}

/// One route of [`Routes`] / [`Explain`].
#[derive(Clone, Serialize)]
pub struct RouteEntry {
    pub controller: String,
    pub handler: String,
    pub method: &'static str,
    pub path: String,
    pub parameters: Vec<ParameterEntry>,
    pub body: BodyEntry,
    pub source: SourceEntry,
}

impl RouteEntry {
    pub fn new(controller: &Controller, route: &Route) -> Self {
        Self::of(&controller.name, route)
    }

    pub fn of(controller_name: &str, route: &Route) -> Self {
        Self {
            controller: controller_name.to_owned(),
            handler: route.method_name.clone(),
            method: route.http_method.as_str(),
            path: match &route.path {
                RoutePath::Static(path) => path.clone(),
                RoutePath::Unresolved => route.path.to_string(),
            },
            parameters: route.parameters.iter().map(ParameterEntry::new).collect(),
            body: BodyEntry::from(&route.body),
            source: SourceEntry::from(&route.source),
        }
    }
}

/// A decorated handler parameter. `name` matches the human presentation:
/// empty for the whole-object form, `<unresolved>` for a non-literal key.
#[derive(Clone, Serialize)]
pub struct ParameterEntry {
    pub kind: &'static str,
    pub name: String,
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub fields: Vec<String>,
}

impl ParameterEntry {
    pub fn new(parameter: &crate::route::RouteParameter) -> Self {
        let kind = match parameter.kind {
            ParameterKind::Param => "param",
            ParameterKind::Query => "query",
            ParameterKind::Body => "body",
            ParameterKind::Headers => "headers",
        };
        let name = match &parameter.name {
            ParameterName::Unnamed => String::new(),
            ParameterName::Named(name) => name.clone(),
            ParameterName::Unresolved => "<unresolved>".to_owned(),
        };
        Self {
            kind,
            name,
            index: parameter.index,
            type_name: parameter.type_name.clone(),
            fields: parameter.fields.clone(),
        }
    }
}

/// The statically known request-body shape. `{"shape": "none"}`,
/// `{"shape": "unknown"}` or `{"shape": "fields", "fields": [...]}`.
#[derive(Clone, Serialize)]
#[serde(tag = "shape", content = "fields", rename_all = "snake_case")]
pub enum BodyEntry {
    None,
    Unknown,
    Fields(Vec<String>),
}

impl From<&BodyShape> for BodyEntry {
    fn from(body: &BodyShape) -> Self {
        match body {
            BodyShape::None => Self::None,
            BodyShape::Unknown => Self::Unknown,
            BodyShape::Fields(fields) => Self::Fields(fields.clone()),
        }
    }
}

/// A position in a source file (1-based line and column).
#[derive(Clone, Serialize)]
pub struct SourceEntry {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl From<&crate::route::SourceLocation> for SourceEntry {
    fn from(source: &crate::route::SourceLocation) -> Self {
        Self {
            file: source.file.display().to_string(),
            line: source.line,
            column: source.column,
        }
    }
}

/// `discover --format json`
#[derive(Serialize)]
pub struct Discover {
    pub files: Vec<String>,
    pub directories_read: usize,
    pub diagnostics: Vec<JsonDiagnostic>,
    pub complete: bool,
}

impl From<&Discovery> for Discover {
    fn from(discovery: &Discovery) -> Self {
        Self {
            files: discovery
                .files
                .iter()
                .map(|file| file.path.display().to_string())
                .collect(),
            directories_read: discovery.directories_read,
            diagnostics: diagnostics_from(&discovery.diagnostics),
            complete: !discovery.is_incomplete(),
        }
    }
}

/// `sync --format json`
#[derive(Serialize, Default)]
pub struct Sync {
    pub dry_run: bool,
    /// `false` for a dry run, when a critical conflict blocked the run, or
    /// for a no-op staged run.
    pub applied: bool,
    pub complete: bool,
    pub staged_changes: Vec<StagedChangeEntry>,
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub deleted: Vec<String>,
    pub preserved: Vec<String>,
    pub conflicts: Vec<ConflictEntry>,
    pub failed: Vec<FailedEntry>,
    pub diagnostics: Vec<JsonDiagnostic>,
}

impl Sync {
    /// Build the document from a synchronization result. Paths are prefixed
    /// with the collection directory, matching the human-readable output.
    pub fn from_result(
        bruno_dir: &std::path::Path,
        result: &SyncResult,
        dry_run: bool,
        complete: bool,
        staged_changes: &[StagedChange],
    ) -> Self {
        let show = |path: &std::path::Path| bruno_dir.join(path).display().to_string();
        Self {
            dry_run,
            applied: result.applied,
            complete,
            staged_changes: staged_changes.iter().map(StagedChangeEntry::new).collect(),
            created: result.created.iter().map(|p| show(p)).collect(),
            updated: result.updated.iter().map(|p| show(p)).collect(),
            unchanged: result.unchanged.iter().map(|p| show(p)).collect(),
            deleted: result.deleted.iter().map(|p| show(p)).collect(),
            preserved: result.preserved.iter().map(|p| show(p)).collect(),
            conflicts: result
                .conflicts
                .iter()
                .map(|c| ConflictEntry {
                    path: show(&c.path),
                    reason: c.reason.code(),
                    description: c.reason.description(),
                    suggestion: c.reason.suggestion(),
                })
                .collect(),
            failed: result
                .failed
                .iter()
                .map(|(path, reason)| FailedEntry {
                    path: show(path),
                    reason: reason.clone(),
                })
                .collect(),
            diagnostics: diagnostics_from(&result.diagnostics),
        }
    }
}

/// One staged change in [`Sync`].
#[derive(Serialize)]
pub struct StagedChangeEntry {
    pub status: &'static str,
    pub path: String,
}

impl StagedChangeEntry {
    fn new(change: &StagedChange) -> Self {
        let status = match change.status {
            StagedStatus::Added => "added",
            StagedStatus::Modified => "modified",
            StagedStatus::Deleted => "deleted",
            StagedStatus::Renamed => "renamed",
            StagedStatus::TypeChanged => "type-changed",
            StagedStatus::Conflicted => "conflicted",
        };
        Self {
            status,
            path: change.path.display().to_string(),
        }
    }
}

/// One conflict in [`Sync`] / [`Verify`].
#[derive(Serialize)]
pub struct ConflictEntry {
    pub path: String,
    pub reason: &'static str,
    pub description: String,
    pub suggestion: &'static str,
}

/// One failed operation in [`Sync`] / [`Verify`].
#[derive(Serialize)]
pub struct FailedEntry {
    pub path: String,
    pub reason: String,
}

/// `verify --format json` (Phase 14.2). The categories follow the
/// synchronization plan: a *missing* request is one that would be created,
/// *outdated* one that would be updated, *stale* one that would be deleted.
/// Conflicts and preserved files follow the existing ownership rules.
#[derive(Serialize, Default)]
pub struct Verify {
    /// `in_sync`, `out_of_sync`, `blocked`, `incomplete` or
    /// `no_staged_changes`.
    pub status: &'static str,
    pub complete: bool,
    pub summary: VerifySummary,
    pub changes: VerifyChanges,
    pub staged_changes: Vec<StagedChangeEntry>,
    pub conflicts: Vec<ConflictEntry>,
    pub failed: Vec<FailedEntry>,
    pub diagnostics: Vec<JsonDiagnostic>,
}

impl Verify {
    pub fn from_result(
        bruno_dir: &std::path::Path,
        result: &SyncResult,
        staged_changes: &[StagedChange],
        status: &'static str,
        complete: bool,
    ) -> Self {
        let show = |path: &std::path::Path| bruno_dir.join(path).display().to_string();
        Self {
            status,
            complete,
            summary: VerifySummary {
                missing: result.created.len(),
                outdated: result.updated.len(),
                stale: result.deleted.len(),
                unchanged: result.unchanged.len(),
                conflicts: result.conflicts.len(),
                preserved: result.preserved.len(),
            },
            changes: VerifyChanges {
                created: result.created.iter().map(|p| show(p)).collect(),
                updated: result.updated.iter().map(|p| show(p)).collect(),
                deleted: result.deleted.iter().map(|p| show(p)).collect(),
            },
            staged_changes: staged_changes.iter().map(StagedChangeEntry::new).collect(),
            conflicts: result
                .conflicts
                .iter()
                .map(|c| ConflictEntry {
                    path: show(&c.path),
                    reason: c.reason.code(),
                    description: c.reason.description(),
                    suggestion: c.reason.suggestion(),
                })
                .collect(),
            failed: result
                .failed
                .iter()
                .map(|(path, reason)| FailedEntry {
                    path: show(path),
                    reason: reason.clone(),
                })
                .collect(),
            diagnostics: diagnostics_from(&result.diagnostics),
        }
    }
}

/// The category counts of a verification.
#[derive(Serialize, Default)]
pub struct VerifySummary {
    /// Requests that would be created (the generated request does not exist).
    pub missing: usize,
    /// Requests that would be updated (they differ from the desired state).
    pub outdated: usize,
    /// Requests that would be deleted (their route no longer exists).
    pub stale: usize,
    pub unchanged: usize,
    pub conflicts: usize,
    pub preserved: usize,
}

/// The pending changes of a verification, as collection-relative paths.
#[derive(Serialize, Default)]
pub struct VerifyChanges {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub deleted: Vec<String>,
}

/// `explain --format json` (Phase 15). `found` is `false` when the selector
/// matched nothing (`matches` empty) or several routes (`matches` holds all
/// of them, so the caller can refine the selection); the error message goes
/// to stderr in both cases.
#[derive(Serialize, Default)]
pub struct Explain {
    pub found: bool,
    pub complete: bool,
    pub route: Option<RouteEntry>,
    pub resolution: Option<ExplainResolution>,
    pub parameters: Vec<ParameterEntry>,
    pub body: Option<BodyEntry>,
    /// Informational boundaries met while deriving the explanation.
    pub limitations: Vec<String>,
    pub trace: Vec<TraceStepEntry>,
    pub matches: Vec<RouteEntry>,
    pub diagnostics: Vec<JsonDiagnostic>,
}

impl Explain {
    /// Build the document for one explained route.
    pub fn from_explanation(explanation: &Explanation) -> Self {
        let route = &explanation.route;
        Self {
            found: true,
            complete: explanation.complete,
            route: Some(RouteEntry::of(&explanation.controller_name, route)),
            resolution: Some(ExplainResolution::from(&explanation.resolution)),
            parameters: route.parameters.iter().map(ParameterEntry::new).collect(),
            body: Some(BodyEntry::from(&route.body)),
            limitations: explanation.limitations.clone(),
            trace: explanation
                .trace
                .iter()
                .map(|step| TraceStepEntry {
                    finding: step.finding.clone(),
                    detail: step.detail.clone(),
                })
                .collect(),
            matches: Vec::new(),
            diagnostics: diagnostics_from(&explanation.warnings),
        }
    }

    /// Build the document for an unmatched or ambiguous selector.
    pub fn unmatched(matches: Vec<RouteEntry>, complete: bool) -> Self {
        Self {
            found: false,
            complete,
            matches,
            ..Self::default()
        }
    }
}

/// The derived route resolution of [`Explain`]. `None` fields are unknown,
/// never guessed.
#[derive(Serialize)]
pub struct ExplainResolution {
    pub controller_prefix: Option<String>,
    pub handler_path: Option<String>,
    pub final_path: Option<String>,
    pub global_prefix: Option<String>,
    pub prefix_applied: bool,
    pub version_segment: Option<String>,
}

impl From<&Resolution> for ExplainResolution {
    fn from(resolution: &Resolution) -> Self {
        Self {
            controller_prefix: resolution.controller_prefix.clone(),
            handler_path: resolution.handler_path.clone(),
            final_path: resolution.final_path.clone(),
            global_prefix: resolution.global_prefix.clone(),
            prefix_applied: resolution.prefix_applied,
            version_segment: resolution.version_segment.clone(),
        }
    }
}

/// One finding of the analysis trace in [`Explain`].
#[derive(Serialize)]
pub struct TraceStepEntry {
    pub finding: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A [`Diagnostic`] as a structured object.
#[derive(Serialize)]
pub struct JsonDiagnostic {
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    pub message: String,
}

/// Convert diagnostics in their existing order.
pub fn diagnostics_from(diagnostics: &[Diagnostic]) -> Vec<JsonDiagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let severity = match diagnostic.severity {
                Severity::Note => "note",
                Severity::Warning => "warning",
                Severity::Error => "error",
            };
            JsonDiagnostic {
                severity,
                path: diagnostic
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                line: diagnostic.position.map(|(line, _)| line),
                column: diagnostic.position.map(|(_, column)| column),
                message: diagnostic.message.clone(),
            }
        })
        .collect()
}
