//! The `explain` command's model (Phase 15): selector parsing, route
//! matching and the explanation built from **existing analysis results**.
//!
//! This is not a second route analyzer: matching and explanation read the
//! route IR that `analyze_files` already produced. The only derivation is
//! the *resolution* section, which splits the composed route path back into
//! its fragments (global prefix, version segment, controller prefix, handler
//! path) using the same composition rules the extractor applies
//! (`compose_path`, `BootstrapConfig::prefix_applies_to`). A fragment that
//! cannot be split off exactly is reported as a limitation, never guessed.

use std::path::{Path, PathBuf};

use crate::diag::Diagnostic;
use crate::nest::app::BootstrapConfig;
use crate::route::{BodyShape, Controller, HttpMethod, ParameterKind, Route, RoutePath};

/// Selects one route of the analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteSelector {
    /// `GET /users/:id` — HTTP method plus the composed route path.
    MethodPath { method: HttpMethod, path: String },
    /// `--controller UsersController --handler findOne`; the controller may
    /// be omitted when the handler name is unique.
    Handler {
        controller: Option<String>,
        handler: String,
    },
}

impl std::fmt::Display for RouteSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MethodPath { method, path } => write!(f, "{} {path}", method.as_str()),
            Self::Handler {
                controller,
                handler,
            } => match controller {
                Some(controller) => write!(f, "{controller}.{handler}"),
                None => write!(f, "{handler}"),
            },
        }
    }
}

/// Parse the positional selector: `METHOD /path` (method case-insensitive).
pub fn parse_method_path(raw: &str) -> Result<RouteSelector, String> {
    let (method_word, path) = raw.split_once(char::is_whitespace).ok_or_else(|| {
        format!("invalid selector {raw:?}: expected 'METHOD /path', e.g. \"GET /users/:id\"")
    })?;
    let method = parse_http_method(method_word)
        .ok_or_else(|| format!("invalid selector {raw:?}: {method_word:?} is not an HTTP method (GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS, ALL)"))?;
    let path = path.trim().to_owned();
    if path.is_empty() || path.split_whitespace().count() != 1 {
        return Err(format!(
            "invalid selector {raw:?}: the route path must be a single path without whitespace"
        ));
    }
    // Normalize: one leading slash, no empty segments (same convention as
    // the route IR).
    let path = crate::route::compose_path(&[&path]);
    Ok(RouteSelector::MethodPath { method, path })
}

fn parse_http_method(word: &str) -> Option<HttpMethod> {
    Some(match word.to_ascii_uppercase().as_str() {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        "ALL" => HttpMethod::All,
        _ => return None,
    })
}

/// A route that matched the selector.
pub type RouteMatch<'a> = (&'a Controller, &'a Route);

/// Every route of `controllers` matching `selector`, in analysis order.
/// `file` (project-relative) restricts the search to one source file.
pub fn find_matches<'a>(
    controllers: &'a [Controller],
    selector: &RouteSelector,
    file: Option<&Path>,
) -> Vec<RouteMatch<'a>> {
    controllers
        .iter()
        .filter(|controller| file.is_none_or(|file| same_file(&controller.source.file, file)))
        .flat_map(|controller| {
            controller
                .routes
                .iter()
                .filter(move |route| route_matches(controller, route, selector))
                .map(move |route| (controller, route))
        })
        .collect()
}

/// Normalize a user-supplied relative path (`./src/x.ts` → `src/x.ts`).
fn normalize_relative(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn same_file(a: &Path, b: &Path) -> bool {
    normalize_relative(a) == normalize_relative(b)
}

fn route_matches(controller: &Controller, route: &Route, selector: &RouteSelector) -> bool {
    match selector {
        RouteSelector::MethodPath { method, path } => {
            route.http_method == *method
                && matches!(&route.path, RoutePath::Static(route_path) if route_path == path)
        }
        RouteSelector::Handler {
            controller: name,
            handler,
        } => {
            route.method_name == *handler
                && name.as_deref().is_none_or(|name| controller.name == name)
        }
    }
}

/// One finding of the analysis trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceStep {
    pub finding: String,
    pub detail: Option<String>,
}

/// How the final route path was assembled, derived from the IR and the
/// bootstrap configuration. Fields stay `None` when they cannot be
/// established exactly.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolution {
    /// The controller's own path prefix (static), if any.
    pub controller_prefix: Option<String>,
    /// The handler's own path fragment, split off the composed path.
    pub handler_path: Option<String>,
    /// The composed route path as the IR stores it.
    pub final_path: Option<String>,
    /// The statically provable global prefix of the application.
    pub global_prefix: Option<String>,
    /// Whether the global prefix was applied to this route (NestJS exclude
    /// rules can keep it off).
    pub prefix_applied: bool,
    /// The URI version segment (`v1`), when the composed path carries one.
    pub version_segment: Option<String>,
}

/// The complete explanation of one route: facts from the IR, the derived
/// resolution, honest findings and the diagnostics of the controller's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    pub controller_name: String,
    pub route: Route,
    pub resolution: Resolution,
    /// Informational boundaries met while deriving the explanation (never
    /// guesses).
    pub limitations: Vec<String>,
    pub trace: Vec<TraceStep>,
    /// Diagnostics belonging to this explanation: the bootstrap warnings and
    /// the analysis diagnostics of the controller's file (file-scoped, not
    /// route-scoped), severity preserved.
    pub warnings: Vec<Diagnostic>,
    /// Whether the analysis covered every path (mirrors `Analysis`).
    pub complete: bool,
}

/// Build the explanation of one matched route from existing results.
pub fn build_explanation(
    controller: &Controller,
    route: &Route,
    bootstrap: &BootstrapConfig,
    analysis_diagnostics: &[Diagnostic],
    complete: bool,
) -> Explanation {
    let (resolution, mut limitations) = derive_resolution(controller, route, bootstrap);
    let warnings: Vec<Diagnostic> = bootstrap
        .diagnostics
        .iter()
        .chain(analysis_diagnostics.iter().filter(|diagnostic| {
            diagnostic
                .path
                .as_ref()
                .is_some_and(|path| same_file(path, &controller.source.file))
        }))
        .cloned()
        .collect();
    let trace = trace_steps(controller, route, &resolution, &warnings);
    limitations.sort();
    limitations.dedup();
    Explanation {
        controller_name: controller.name.clone(),
        route: route.clone(),
        resolution,
        limitations,
        trace,
        warnings,
        complete,
    }
}

/// Split the composed path back into its fragments. The composition order is
/// `global prefix? / version? / controller path / handler path`; candidates
/// are verified against the bootstrap configuration, and an ambiguous split
/// is reported as a limitation instead of a guess.
fn derive_resolution(
    controller: &Controller,
    route: &Route,
    bootstrap: &BootstrapConfig,
) -> (Resolution, Vec<String>) {
    let mut limitations = Vec::new();
    let controller_prefix = match &controller.path {
        RoutePath::Static(path) => Some(path.clone()),
        RoutePath::Unresolved => {
            limitations.push(
                "the controller path is not a static string; the route path cannot be split into fragments"
                    .to_owned(),
            );
            None
        }
    };
    let final_path = match &route.path {
        RoutePath::Static(path) => Some(path.clone()),
        RoutePath::Unresolved => {
            limitations.push(
                "the route path is not a static string (a dynamic expression); no fragment split is possible"
                    .to_owned(),
            );
            None
        }
    };
    let mut resolution = Resolution {
        controller_prefix,
        handler_path: None,
        final_path,
        global_prefix: bootstrap.global_prefix.clone(),
        prefix_applied: false,
        version_segment: None,
    };
    let (Some(final_path), Some(controller_prefix)) =
        (&resolution.final_path, &resolution.controller_prefix)
    else {
        return (resolution, limitations);
    };

    let final_segments: Vec<&str> = segments(final_path);
    let controller_segments: Vec<&str> = segments(controller_prefix);
    let prefix_segments: Vec<&str> = bootstrap
        .global_prefix
        .as_deref()
        .map(segments)
        .unwrap_or_default();
    let prefix_len = prefix_segments.len();

    // Candidates: (prefix applied, version segment, handler fragment). The
    // composition order is prefix, then version, then controller path, then
    // handler path.
    let mut candidates: Vec<(bool, Option<String>, String)> = Vec::new();
    let prefix_variants: &[(bool, usize)] = if prefix_len > 0 {
        &[(true, prefix_len), (false, 0)]
    } else {
        &[(false, 0)]
    };
    for (applied, skip) in prefix_variants.iter().copied() {
        if final_segments.len() < skip {
            continue;
        }
        // With the prefix applied, the path must actually start with it.
        if applied && final_segments[..skip] != prefix_segments[..] {
            continue;
        }
        let rest = &final_segments[skip..];
        for version_take in [0usize, 1] {
            let Some((version, after_version)) = take_version(rest, version_take) else {
                continue;
            };
            let Some(handler_segments) = after_version.strip_prefix_segments(&controller_segments)
            else {
                continue;
            };
            let local = compose(&controller_segments, handler_segments);
            let prefix_should_apply = bootstrap.prefix_applies_to(&local, route.http_method);
            // Consistency with the bootstrap configuration decides.
            if applied != prefix_should_apply {
                continue;
            }
            candidates.push((applied, version, compose(&[], handler_segments)));
        }
    }

    match candidates.as_slice() {
        [(applied, version, handler)] => {
            resolution.prefix_applied = *applied;
            resolution.version_segment = version.clone();
            resolution.handler_path = Some(handler.clone());
        }
        [] => limitations.push(
            "the composed path cannot be split into prefix/version/controller/handler fragments exactly; the resolution segments are unknown"
                .to_owned(),
        ),
        _ => limitations.push(
            "several fragment splits of the composed path are consistent with the configuration; the exact split is ambiguous"
                .to_owned(),
        ),
    }
    (resolution, limitations)
}

/// `take == 0`: no version segment. `take == 1`: the first segment must be a
/// NestJS version segment (`v<number>`).
fn take_version<'a>(
    segments: &'a [&'a str],
    take: usize,
) -> Option<(Option<String>, &'a [&'a str])> {
    match take {
        0 => Some((None, segments)),
        _ => {
            let (first, rest) = segments.split_first()?;
            if is_version_segment(first) {
                Some((Some((*first).to_owned()), rest))
            } else {
                None
            }
        }
    }
}

trait StripPrefixSegments {
    fn strip_prefix_segments(&self, prefix: &[&str]) -> Option<&[&str]>;
}

impl StripPrefixSegments for [&str] {
    fn strip_prefix_segments(&self, prefix: &[&str]) -> Option<&[&str]> {
        if self.len() >= prefix.len() && self[..prefix.len()] == *prefix {
            Some(&self[prefix.len()..])
        } else {
            None
        }
    }
}

fn is_version_segment(segment: &str) -> bool {
    segment
        .strip_prefix('v')
        .is_some_and(|version| version.parse::<u32>().is_ok())
}

fn segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn compose(fragments: &[&str], more: &[&str]) -> String {
    crate::route::compose_path(&[&fragments.join("/"), &more.join("/")])
}

/// The derived findings checklist — every step states evidence that exists.
fn trace_steps(
    controller: &Controller,
    route: &Route,
    resolution: &Resolution,
    warnings: &[Diagnostic],
) -> Vec<TraceStep> {
    let at = |source: &crate::route::SourceLocation| {
        format!(
            "{}:{}:{}",
            source.file.display(),
            source.line,
            source.column
        )
    };
    let mut steps = vec![TraceStep {
        finding: "Controller identified".to_owned(),
        detail: Some(format!(
            "{} (at {})",
            controller.name,
            at(&controller.source)
        )),
    }];
    steps.push(match &controller.path {
        RoutePath::Static(path) if path == "/" => TraceStep {
            finding: "Controller prefix".to_owned(),
            detail: Some("none (the controller has no path prefix)".to_owned()),
        },
        RoutePath::Static(path) => TraceStep {
            finding: "Controller prefix".to_owned(),
            detail: Some(path.clone()),
        },
        RoutePath::Unresolved => TraceStep {
            finding: "Controller prefix".to_owned(),
            detail: Some("unresolved (the @Controller path is not a static string)".to_owned()),
        },
    });
    steps.push(TraceStep {
        finding: "Handler identified".to_owned(),
        detail: Some(format!(
            "{} (route decorator at {})",
            route.method_name,
            at(&route.source)
        )),
    });
    steps.push(TraceStep {
        finding: "HTTP method resolved".to_owned(),
        detail: Some(format!(
            "{} (from the route decorator at {})",
            route.http_method.as_str(),
            at(&route.source)
        )),
    });
    steps.push(match &resolution.final_path {
        Some(final_path) => {
            let mut parts = Vec::new();
            if resolution.prefix_applied {
                parts.push(format!(
                    "global prefix {}",
                    resolution.global_prefix.as_deref().unwrap_or_default()
                ));
            }
            if let Some(version) = &resolution.version_segment {
                parts.push(format!("version {version}"));
            }
            if let Some(prefix) = &resolution.controller_prefix
                && prefix != "/"
            {
                parts.push(format!("controller path {prefix}"));
            }
            if let Some(handler) = &resolution.handler_path
                && handler != "/"
            {
                parts.push(format!("handler path {handler}"));
            }
            let detail = if parts.is_empty() {
                final_path.clone()
            } else {
                format!("{} → {final_path}", parts.join(" + "))
            };
            TraceStep {
                finding: "Route path composed".to_owned(),
                detail: Some(detail),
            }
        }
        None => TraceStep {
            finding: "Route path composed".to_owned(),
            detail: Some("unresolved (a dynamic expression; never guessed)".to_owned()),
        },
    });
    if route.parameters.is_empty() {
        steps.push(TraceStep {
            finding: "Parameters extracted".to_owned(),
            detail: Some("none".to_owned()),
        });
    } else {
        let count = |kind: ParameterKind| {
            route
                .parameters
                .iter()
                .filter(|parameter| parameter.kind == kind)
                .count()
        };
        steps.push(TraceStep {
            finding: "Parameters extracted".to_owned(),
            detail: Some(format!(
                "{} (param: {}, query: {}, body: {}, headers: {})",
                route.parameters.len(),
                count(ParameterKind::Param),
                count(ParameterKind::Query),
                count(ParameterKind::Body),
                count(ParameterKind::Headers),
            )),
        });
    }
    steps.push(TraceStep {
        finding: "Request body shape".to_owned(),
        detail: Some(match &route.body {
            BodyShape::None => "none (no @Body parameter)".to_owned(),
            BodyShape::Unknown => {
                "unknown (the body type could not be resolved statically)".to_owned()
            }
            BodyShape::Fields(fields) => format!("fields: {}", fields.join(", ")),
        }),
    });
    let (errors, others) =
        warnings
            .iter()
            .fold((0usize, 0usize), |(errors, others), diagnostic| {
                if diagnostic.severity == crate::diag::Severity::Error {
                    (errors + 1, others)
                } else {
                    (errors, others + 1)
                }
            });
    steps.push(TraceStep {
        finding: "Diagnostics affecting this explanation".to_owned(),
        detail: Some(if warnings.is_empty() {
            "none".to_owned()
        } else {
            format!("{errors} error(s), {others} warning(s)/note(s) — see Warnings")
        }),
    });
    steps
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::*;
    use crate::nest::app::analyze_bootstrap;
    use crate::route::{ParameterName, Route, RouteParameter, SourceLocation};

    fn location(file: &str) -> SourceLocation {
        SourceLocation {
            file: Arc::from(Path::new(file)),
            line: 3,
            column: 1,
        }
    }

    fn controller(path: &str, routes: Vec<Route>) -> Controller {
        named_controller("UsersController", path, routes)
    }

    fn named_controller(name: &str, path: &str, routes: Vec<Route>) -> Controller {
        Controller {
            name: name.to_owned(),
            path: if path.is_empty() {
                RoutePath::Unresolved
            } else {
                RoutePath::Static(path.to_owned())
            },
            source: location("src/users/users.controller.ts"),
            routes,
        }
    }

    fn route(handler: &str, method: HttpMethod, path: &str) -> Route {
        Route {
            method_name: handler.to_owned(),
            http_method: method,
            path: RoutePath::Static(path.to_owned()),
            parameters: vec![],
            body: BodyShape::None,
            source: location("src/users/users.controller.ts"),
        }
    }

    fn bootstrap(source: &str) -> BootstrapConfig {
        analyze_bootstrap(source, Path::new("src/main.ts"))
    }

    const PLAIN: &str = "async function bootstrap() {}";
    const PREFIXED: &str = r#"
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api');
}
bootstrap();
"#;
    const PREFIXED_EXCLUDING: &str = r#"
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api', { exclude: ['health'] });
}
bootstrap();
"#;
    const VERSIONED: &str = r#"
import { VersioningType } from '@nestjs/common';
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api');
  app.enableVersioning({ type: VersioningType.URI, defaultVersion: '1' });
}
bootstrap();
"#;

    // ---- selector parsing

    #[test]
    fn parses_method_path_selectors() {
        let selector = parse_method_path("GET /users/:id").unwrap();
        assert_eq!(
            selector,
            RouteSelector::MethodPath {
                method: HttpMethod::Get,
                path: "/users/:id".to_owned()
            }
        );
        // case-insensitive method, missing and extra slashes normalize
        let selector = parse_method_path("post   auth/login/").unwrap();
        assert_eq!(
            selector,
            RouteSelector::MethodPath {
                method: HttpMethod::Post,
                path: "/auth/login".to_owned()
            }
        );
        let selector = parse_method_path("all /").unwrap();
        assert_eq!(
            selector,
            RouteSelector::MethodPath {
                method: HttpMethod::All,
                path: "/".to_owned()
            }
        );
    }

    #[test]
    fn rejects_malformed_selectors() {
        for raw in ["GET", "GET ", "/users", "FETCH /users", "GET", ""] {
            assert!(parse_method_path(raw).is_err(), "{raw:?}");
        }
        let message = parse_method_path("FETCH /users").unwrap_err();
        assert!(message.contains("not an HTTP method"), "{message}");
    }

    // ---- matching

    #[test]
    fn matches_by_method_and_path() {
        let controllers = [controller(
            "/users",
            vec![
                route("findAll", HttpMethod::Get, "/users"),
                route("findOne", HttpMethod::Get, "/users/:id"),
            ],
        )];
        let selector = parse_method_path("get /USERS/:id".to_lowercase().as_str())
            .unwrap_or_else(|_| parse_method_path("get /users/:id").unwrap());
        let matches = find_matches(&controllers, &selector, None);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1.method_name, "findOne");

        let selector = parse_method_path("DELETE /users").unwrap();
        assert!(find_matches(&controllers, &selector, None).is_empty());
    }

    #[test]
    fn matches_by_controller_and_handler() {
        let controllers = [
            named_controller(
                "UsersController",
                "/users",
                vec![route("findAll", HttpMethod::Get, "/users")],
            ),
            named_controller(
                "AuthController",
                "/auth",
                vec![route("findAll", HttpMethod::Post, "/auth")],
            ),
        ];
        let selector = RouteSelector::Handler {
            controller: Some("AuthController".to_owned()),
            handler: "findAll".to_owned(),
        };
        let matches = find_matches(&controllers, &selector, None);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0.name, "AuthController");

        let ambiguous = RouteSelector::Handler {
            controller: None,
            handler: "findAll".to_owned(),
        };
        assert_eq!(find_matches(&controllers, &ambiguous, None).len(), 2);
    }

    #[test]
    fn the_file_filter_restricts_the_search() {
        let controllers = [controller(
            "/users",
            vec![route("findAll", HttpMethod::Get, "/users")],
        )];
        let selector = RouteSelector::Handler {
            controller: None,
            handler: "findAll".to_owned(),
        };
        assert_eq!(
            find_matches(
                &controllers,
                &selector,
                Some(Path::new("src/users/users.controller.ts"))
            )
            .len(),
            1
        );
        assert_eq!(
            find_matches(
                &controllers,
                &selector,
                Some(Path::new("./src/users/users.controller.ts"))
            )
            .len(),
            1,
            "the file filter normalizes ./ prefixes"
        );
        assert!(find_matches(&controllers, &selector, Some(Path::new("src/other.ts"))).is_empty());
    }

    // ---- resolution derivation

    fn resolution_of(
        controller_path: &str,
        final_path: &str,
        config: &BootstrapConfig,
    ) -> Resolution {
        let controllers = controller(
            controller_path,
            vec![route("findOne", HttpMethod::Get, final_path)],
        );
        let (resolution, limitations) =
            derive_resolution(&controllers, &controllers.routes[0], config);
        assert!(limitations.is_empty(), "{limitations:?}");
        resolution
    }

    #[test]
    fn splits_a_plain_route_into_prefix_and_handler() {
        let config = bootstrap(PLAIN);
        let resolution = resolution_of("/users", "/users/:id", &config);
        assert_eq!(resolution.controller_prefix.as_deref(), Some("/users"));
        assert_eq!(resolution.handler_path.as_deref(), Some("/:id"));
        assert!(!resolution.prefix_applied);
        assert_eq!(resolution.version_segment, None);
    }

    #[test]
    fn splits_off_the_global_prefix_and_reports_it_applied() {
        let config = bootstrap(PREFIXED);
        let resolution = resolution_of("/users", "/api/users/:id", &config);
        assert_eq!(resolution.handler_path.as_deref(), Some("/:id"));
        assert!(resolution.prefix_applied);
        assert_eq!(resolution.global_prefix.as_deref(), Some("api"));
    }

    #[test]
    fn excludes_keep_the_prefix_off_the_route() {
        let config = bootstrap(PREFIXED_EXCLUDING);
        let resolution = resolution_of("/health", "/health", &config);
        assert_eq!(resolution.handler_path.as_deref(), Some("/"));
        assert!(!resolution.prefix_applied);
        let resolution = resolution_of("/users", "/api/users", &config);
        assert!(resolution.prefix_applied);
        assert_eq!(resolution.handler_path.as_deref(), Some("/"));
    }

    #[test]
    fn splits_off_the_version_segment() {
        let config = bootstrap(VERSIONED);
        let resolution = resolution_of("/users", "/api/v1/users/:id", &config);
        assert!(resolution.prefix_applied);
        assert_eq!(resolution.version_segment.as_deref(), Some("v1"));
        assert_eq!(resolution.handler_path.as_deref(), Some("/:id"));
    }

    #[test]
    fn a_controller_without_prefix_leaves_the_whole_tail_as_handler() {
        let config = bootstrap(PLAIN);
        let resolution = resolution_of("/", "/:id", &config);
        assert_eq!(resolution.controller_prefix.as_deref(), Some("/"));
        assert_eq!(resolution.handler_path.as_deref(), Some("/:id"));
    }

    #[test]
    fn unresolved_paths_stay_unresolved_with_a_limitation() {
        let config = bootstrap(PLAIN);
        let c = Controller {
            name: "UsersController".to_owned(),
            path: RoutePath::Unresolved,
            source: location("src/users/users.controller.ts"),
            routes: vec![],
        };
        let mut r = route("findOne", HttpMethod::Get, "");
        r.path = RoutePath::Unresolved;
        let (resolution, limitations) = derive_resolution(&c, &r, &config);
        assert_eq!(resolution.controller_prefix, None);
        assert_eq!(resolution.final_path, None);
        assert_eq!(resolution.handler_path, None);
        assert_eq!(limitations.len(), 2, "{limitations:?}");
    }

    #[test]
    fn a_ambiguous_split_is_a_limitation_not_a_guess() {
        // The composed path does not contain the controller path at all.
        let config = bootstrap(PLAIN);
        let controllers = controller(
            "/users",
            vec![route("findOne", HttpMethod::Get, "/something/else")],
        );
        let (resolution, limitations) =
            derive_resolution(&controllers, &controllers.routes[0], &config);
        assert_eq!(resolution.handler_path, None);
        assert!(!limitations.is_empty());
    }

    // ---- explanation build

    #[test]
    fn the_explanation_collects_file_scoped_warnings_and_findings() {
        let config = bootstrap(PLAIN);
        let controllers = controller(
            "/users",
            vec![route("findOne", HttpMethod::Get, "/users/:id")],
        );
        let diagnostics = vec![
            Diagnostic::new(
                crate::diag::Severity::Warning,
                Some(PathBuf::from("src/users/users.controller.ts")),
                "odd thing",
            ),
            Diagnostic::new(
                crate::diag::Severity::Warning,
                Some(PathBuf::from("src/other.ts")),
                "elsewhere",
            ),
        ];
        let explanation = build_explanation(
            &controllers,
            &controllers.routes[0],
            &config,
            &diagnostics,
            true,
        );
        assert_eq!(
            explanation.warnings.len(),
            1,
            "only this file's diagnostics"
        );
        assert_eq!(explanation.warnings[0].message, "odd thing");
        assert!(explanation.complete);
        let findings: Vec<&str> = explanation
            .trace
            .iter()
            .map(|step| step.finding.as_str())
            .collect();
        assert_eq!(
            findings,
            [
                "Controller identified",
                "Controller prefix",
                "Handler identified",
                "HTTP method resolved",
                "Route path composed",
                "Parameters extracted",
                "Request body shape",
                "Diagnostics affecting this explanation",
            ]
        );
        let path_step = &explanation.trace[4];
        assert_eq!(
            path_step.detail.as_deref(),
            Some("controller path /users + handler path /:id → /users/:id")
        );
    }

    #[test]
    fn parameter_counts_are_reported_by_kind() {
        let config = bootstrap(PLAIN);
        let mut r = route("findOne", HttpMethod::Get, "/users/:id");
        r.parameters.push(RouteParameter {
            kind: ParameterKind::Param,
            name: ParameterName::Named("id".to_owned()),
            index: 0,
            type_name: Some("string".to_owned()),
            fields: vec![],
        });
        r.parameters.push(RouteParameter {
            kind: ParameterKind::Query,
            name: ParameterName::Unnamed,
            index: 1,
            type_name: None,
            fields: vec!["page".to_owned()],
        });
        let controllers = controller("/users", vec![r]);
        let explanation =
            build_explanation(&controllers, &controllers.routes[0], &config, &[], true);
        let step = &explanation.trace[5];
        assert_eq!(
            step.detail.as_deref(),
            Some("2 (param: 1, query: 1, body: 0, headers: 0)")
        );
    }
}
