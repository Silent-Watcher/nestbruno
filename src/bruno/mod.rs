//! Bruno collection generation.
//!
//! ```text
//! Route IR ─▶ model (BrunoRequest) ─▶ serialize (.bru text) ─▶ GeneratedRequest
//!                                                                    │
//!                              writer (safe, atomic filesystem writes) ◀┘
//! ```
//!
//! [`generate`] is a pure function: it computes the *desired* files from the
//! route IR and never touches the filesystem, never parses TypeScript and never
//! reads existing Bruno files. [`writer`] is the only module that writes.
//!
//! # Layout
//!
//! ```text
//! <bruno>/
//! ├── bruno.json              created once if absent, never overwritten
//! └── <Controller>/           one folder per controller (see `naming`)
//!     └── <handler>.bru       one file per route
//! ```
//!
//! No `folder.bru` is generated: Bruno shows the directory name, and a
//! `folder.bru` would be a file users customize (headers, auth, scripts).

pub mod identity;
pub mod model;
pub mod naming;
pub mod serialize;
pub mod stamp;
pub mod writer;

use crate::config::Config;
use crate::diag::{Diagnostic, Severity};
use crate::route::Controller;
use identity::{ControllerIdentity, RouteIdentity};
use model::build_request;
use serialize::serialize;

/// One `.bru` file that should exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRequest {
    /// Folder name (a single safe path component).
    pub folder: String,
    /// File name including `.bru` (a single safe path component).
    pub file_name: String,
    pub identity: RouteIdentity,
    /// Complete file content.
    pub content: String,
    /// The stamp line contained in `content`, kept so that consumers do not
    /// have to parse it back out of the text.
    pub stamp: stamp::Stamp,
}

/// The desired set of request files plus what could not be generated.
#[derive(Debug, Default)]
pub struct Generation {
    /// In controller order, then route (source) order.
    pub requests: Vec<GeneratedRequest>,
    /// Routes or parameters that were left out, each with the reason.
    pub diagnostics: Vec<Diagnostic>,
    /// Routes that exist in the source but produced no request (unsupported
    /// or unsafe). Synchronization must not treat their existing requests as
    /// stale.
    pub skipped: Vec<RouteIdentity>,
}

/// Compute the desired Bruno files for `controllers`.
///
/// Controllers without routes produce nothing. Routes that cannot be
/// represented (`@All`, unresolved or unsafe paths) are skipped with a
/// warning; they never abort generation of the others.
pub fn generate(config: &Config, controllers: &[Controller]) -> Generation {
    let mut generation = Generation::default();
    let folders = folder_names(controllers.iter());
    for (controller, folder) in controllers.iter().zip(folders) {
        if let Some(folder) = folder {
            generate_controller(config, controller, &folder, &mut generation);
        }
    }
    generation
}

/// The folder name of every controller, in input order: `None` for controllers
/// without routes (they produce no folder).
///
/// This is the only *global* part of generation (names are made distinct among
/// all controllers), and it is cheap: no routes are looked at. The expensive
/// per-controller work is [`generate_controller`], which depends only on the
/// controller and its folder name, so callers may cache it.
pub fn folder_names<'a>(
    controllers: impl Iterator<Item = &'a Controller> + Clone,
) -> Vec<Option<String>> {
    let candidates: Vec<(String, u64)> = controllers
        .clone()
        .filter(|c| !c.routes.is_empty())
        .map(|c| {
            (
                naming::folder_name(&c.name),
                ControllerIdentity::of(c).fingerprint(),
            )
        })
        .collect();
    let mut resolved = naming::resolve_collisions(&candidates).into_iter();
    controllers
        .map(|c| {
            if c.routes.is_empty() {
                None
            } else {
                resolved.next()
            }
        })
        .collect()
}

struct Pending {
    identity: RouteIdentity,
    content: String,
    stamp: stamp::Stamp,
}

/// Generate the requests of one controller into `generation`.
pub fn generate_controller(
    config: &Config,
    controller: &Controller,
    folder: &str,
    generation: &mut Generation,
) {
    let mut pending: Vec<Pending> = Vec::new();

    for route in &controller.routes {
        let seq = pending.len() + 1;
        let skip = |generation: &mut Generation, severity, message: String| {
            generation.diagnostics.push(
                Diagnostic::new(
                    severity,
                    Some(route.source.file.to_path_buf()),
                    format!("{}.{}: {message}", controller.name, route.method_name),
                )
                .at(route.source.line, route.source.column),
            );
        };

        let built = match build_request(&config.base_url, controller, route, seq) {
            Ok(built) => built,
            Err(reason) => {
                generation
                    .skipped
                    .push(RouteIdentity::of(controller, route));
                skip(
                    generation,
                    Severity::Warning,
                    format!("no Bruno request generated: {reason}"),
                );
                continue;
            }
        };
        for warning in &built.warnings {
            skip(generation, Severity::Warning, warning.clone());
        }
        let identity = RouteIdentity::of(controller, route);
        let content = serialize(&built.request)
            .map_err(|err| err.to_string())
            .and_then(|text| stamp::stamp_with(&text, &identity).map_err(str::to_owned));
        match content {
            Ok((content, stamp)) => pending.push(Pending {
                identity,
                content,
                stamp,
            }),
            Err(err) => {
                generation.skipped.push(identity);
                skip(
                    generation,
                    Severity::Error,
                    format!("no Bruno request generated: {err}"),
                );
            }
        }
    }

    let candidates: Vec<(String, u64)> = pending
        .iter()
        .map(|p| {
            (
                naming::file_stem(&p.identity.handler),
                p.identity.fingerprint(),
            )
        })
        .collect();
    let stems = naming::resolve_collisions(&candidates);
    for (pending, stem) in pending.into_iter().zip(stems) {
        generation.requests.push(GeneratedRequest {
            folder: folder.to_owned(),
            file_name: format!("{stem}.bru"),
            identity: pending.identity,
            content: pending.content,
            stamp: pending.stamp,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::route::{
        BodyShape, HttpMethod, ParameterKind, ParameterName, Route, RouteParameter, RoutePath,
        SourceLocation,
    };

    fn location(file: &str, line: u32) -> SourceLocation {
        SourceLocation {
            file: Arc::from(Path::new(file)),
            line,
            column: 3,
        }
    }

    fn route(method: HttpMethod, handler: &str, path: &str) -> Route {
        Route {
            method_name: handler.to_owned(),
            http_method: method,
            path: RoutePath::Static(path.to_owned()),
            parameters: vec![],
            body: BodyShape::None,
            source: location("src/a.controller.ts", 10),
        }
    }

    fn controller(name: &str, file: &str, routes: Vec<Route>) -> Controller {
        Controller {
            name: name.to_owned(),
            path: RoutePath::Static("/".into()),
            source: location(file, 1),
            routes,
        }
    }

    fn names(generation: &Generation) -> Vec<String> {
        generation
            .requests
            .iter()
            .map(|r| format!("{}/{}", r.folder, r.file_name))
            .collect()
    }

    fn users() -> Controller {
        controller(
            "UsersController",
            "src/users/users.controller.ts",
            vec![
                route(HttpMethod::Get, "findAll", "/users"),
                route(HttpMethod::Get, "findOne", "/users/:id"),
                route(HttpMethod::Post, "create", "/users"),
            ],
        )
    }

    #[test]
    fn controllers_map_to_folders_and_routes_to_files() {
        let auth = controller(
            "AuthController",
            "src/auth/auth.controller.ts",
            vec![route(HttpMethod::Post, "login", "/auth/login")],
        );
        let g = generate(&Config::default(), &[users(), auth]);
        assert_eq!(
            names(&g),
            [
                "Users/find-all.bru",
                "Users/find-one.bru",
                "Users/create.bru",
                "Auth/login.bru"
            ]
        );
        assert!(g.diagnostics.is_empty());
        assert!(g.requests[1].content.contains("url: {{baseUrl}}/users/:id"));
        assert!(
            g.requests[2]
                .content
                .starts_with("meta {\n  name: POST /users\n  type: http\n  seq: 3\n}")
        );
    }

    #[test]
    fn controllers_without_routes_produce_nothing() {
        let g = generate(
            &Config::default(),
            &[controller("EmptyController", "a.ts", vec![])],
        );
        assert!(g.requests.is_empty() && g.diagnostics.is_empty());
    }

    #[test]
    fn base_url_from_the_configuration_is_used() {
        let config = Config {
            base_url: "http://localhost:3000".into(),
            ..Config::default()
        };
        let g = generate(&config, &[users()]);
        assert!(
            g.requests[0]
                .content
                .contains("url: http://localhost:3000/users\n")
        );
    }

    #[test]
    fn routes_with_similar_paths_never_share_a_file() {
        let c = controller(
            "UsersController",
            "src/u.ts",
            vec![
                route(HttpMethod::Get, "getUser", "/users/:id"),
                route(HttpMethod::Get, "getUserByUserId", "/users/:userId"),
                route(HttpMethod::Post, "postUser", "/users/:id"),
                route(HttpMethod::Get, "profile", "/users/profile"),
                route(HttpMethod::Get, "profileSlash", "/users/profile/"),
            ],
        );
        let g = generate(&Config::default(), &[c]);
        let n = names(&g);
        let distinct: std::collections::HashSet<_> = n.iter().map(|s| s.to_lowercase()).collect();
        assert_eq!(distinct.len(), 5, "{n:?}");
    }

    #[test]
    fn handlers_that_slug_alike_are_all_suffixed_deterministically() {
        let c = controller(
            "C",
            "src/c.ts",
            vec![
                route(HttpMethod::Get, "findOne", "/a"),
                route(HttpMethod::Get, "find_one", "/b"),
                route(HttpMethod::Get, "other", "/c"),
            ],
        );
        let g = generate(&Config::default(), std::slice::from_ref(&c));
        let n = names(&g);
        assert!(
            n[0].starts_with("C/find-one-") && n[1].starts_with("C/find-one-"),
            "{n:?}"
        );
        assert_ne!(n[0], n[1]);
        assert_eq!(n[2], "C/other.bru");
        assert_eq!(names(&generate(&Config::default(), &[c])), n);
    }

    #[test]
    fn same_named_controllers_in_different_files_get_distinct_folders() {
        let a = controller(
            "UsersController",
            "src/admin/users.controller.ts",
            vec![route(HttpMethod::Get, "list", "/admin/users")],
        );
        let b = controller(
            "UsersController",
            "src/public/users.controller.ts",
            vec![route(HttpMethod::Get, "list", "/users")],
        );
        let forward = names(&generate(&Config::default(), &[a.clone(), b.clone()]));
        let backward = names(&generate(&Config::default(), &[b, a]));
        assert!(forward[0].starts_with("Users-") && forward[1].starts_with("Users-"));
        assert_ne!(forward[0], forward[1]);
        // Order independence: the same controller keeps the same folder.
        assert_eq!(forward[0], backward[1]);
        assert_eq!(forward[1], backward[0]);
    }

    #[test]
    fn all_routes_are_skipped_with_a_warning_and_others_are_kept() {
        let c = controller(
            "C",
            "src/c.ts",
            vec![
                route(HttpMethod::All, "any", "/x"),
                route(HttpMethod::Get, "ok", "/y"),
            ],
        );
        let g = generate(&Config::default(), &[c]);
        assert_eq!(names(&g), ["C/ok.bru"]);
        assert_eq!(g.diagnostics.len(), 1);
        let d = &g.diagnostics[0];
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.path.as_deref(), Some(Path::new("src/a.controller.ts")));
        assert!(
            d.message.starts_with("C.any: no Bruno request generated")
                && d.message.contains("@All")
        );
        assert!(
            g.requests[0].content.contains("seq: 1"),
            "skipped routes do not consume a seq"
        );
    }

    #[test]
    fn every_route_that_produces_no_request_is_recorded_as_skipped() {
        // Regression: skipped routes must be reported so that synchronization
        // never mistakes "could not generate" for "route removed".
        let mut unresolved = route(HttpMethod::Get, "u", "/x");
        unresolved.path = RoutePath::Unresolved;
        let c = controller(
            "C",
            "src/c.ts",
            vec![
                route(HttpMethod::All, "any", "/a"),
                unresolved,
                route(HttpMethod::Get, "hostile", "/a\nb"),
                route(HttpMethod::Get, "ok", "/ok"),
            ],
        );
        let g = generate(&Config::default(), &[c]);
        assert_eq!(g.requests.len(), 1);
        let handlers: Vec<_> = g.skipped.iter().map(|i| i.handler.as_str()).collect();
        assert_eq!(handlers, ["any", "u", "hostile"]);
    }

    #[test]
    fn unresolved_and_hostile_routes_are_skipped_not_fatal() {
        let mut unresolved = route(HttpMethod::Get, "u", "/x");
        unresolved.path = RoutePath::Unresolved;
        let c = controller(
            "C",
            "src/c.ts",
            vec![
                unresolved,
                route(
                    HttpMethod::Get,
                    "inject",
                    "/a\n}\n\nscript:pre-request {\n  evil()\n}",
                ),
                route(HttpMethod::Get, "env", "/{{process.env.SECRET}}"),
                route(HttpMethod::Get, "ok", "/ok"),
            ],
        );
        let g = generate(&Config::default(), &[c]);
        assert_eq!(names(&g), ["C/ok.bru"]);
        assert_eq!(g.diagnostics.len(), 3);
        assert!(
            g.requests
                .iter()
                .all(|r| !r.content.contains("script:") && !r.content.contains("process.env"))
        );
    }

    #[test]
    fn hostile_class_and_method_names_cannot_reach_the_filesystem_layout() {
        let c = controller(
            "../../etc/passwdController",
            "src/c.ts",
            vec![
                route(HttpMethod::Get, "../../../escape", "/a"),
                route(HttpMethod::Get, "con", "/b"),
                route(HttpMethod::Get, "کاربران", "/c"),
                route(HttpMethod::Get, &"x".repeat(1000), "/d"),
            ],
        );
        let g = generate(&Config::default(), &[c]);
        assert_eq!(g.requests.len(), 4);
        for r in &g.requests {
            for part in [&r.folder, &r.file_name] {
                assert!(
                    !part.contains(['/', '\\', ':']) && part != ".." && part != ".",
                    "{part}"
                );
                assert!(part.len() < 80, "{part}");
            }
        }
        assert_eq!(g.requests[0].folder, "etcpasswd");
        assert_eq!(g.requests[1].file_name, "con_.bru");
        assert_eq!(g.requests[2].file_name, "route.bru");
    }

    #[test]
    fn parameters_flow_through_to_the_file() {
        let mut r = route(HttpMethod::Post, "update", "/users/:id");
        r.parameters = vec![
            RouteParameter {
                kind: ParameterKind::Param,
                name: ParameterName::Named("id".into()),
                index: 0,
                type_name: None,
                fields: Vec::new(),
            },
            RouteParameter {
                kind: ParameterKind::Query,
                name: ParameterName::Named("notify".into()),
                index: 1,
                type_name: None,
                fields: Vec::new(),
            },
            RouteParameter {
                kind: ParameterKind::Body,
                name: ParameterName::Unnamed,
                index: 2,
                type_name: None,
                fields: Vec::new(),
            },
            RouteParameter {
                kind: ParameterKind::Headers,
                name: ParameterName::Named("x\ny".into()),
                index: 3,
                type_name: None,
                fields: Vec::new(),
            },
        ];
        let g = generate(
            &Config::default(),
            &[controller("UsersController", "src/u.ts", vec![r])],
        );
        let content = &g.requests[0].content;
        assert!(content.contains("params:query {\n  ~notify: \n}"));
        assert!(content.contains("params:path {\n  id: \n}"));
        assert!(content.contains("body: json") && content.contains("body:json {\n  {}\n}"));
        assert!(!content.contains("headers {"));
        assert_eq!(g.diagnostics.len(), 1, "the unsafe header name is reported");
    }

    #[test]
    fn generation_is_deterministic() {
        let controllers = [users()];
        let first = generate(&Config::default(), &controllers);
        let second = generate(&Config::default(), &controllers);
        assert_eq!(first.requests, second.requests);
    }

    #[test]
    fn many_routes_are_generated_without_collisions() {
        let routes: Vec<Route> = (0..500)
            .map(|i| {
                route(
                    HttpMethod::Get,
                    &format!("handler{}", i % 50),
                    &format!("/r{i}"),
                )
            })
            .collect();
        let cs: Vec<Controller> = (0..4)
            .map(|c| {
                controller(
                    "SameController",
                    &format!("src/m{c}/same.controller.ts"),
                    routes[c * 100..c * 100 + 100].to_vec(),
                )
            })
            .collect();
        let g = generate(&Config::default(), &cs);
        assert_eq!(g.requests.len(), 400);
        let distinct: std::collections::HashSet<_> =
            names(&g).iter().map(|s| s.to_lowercase()).collect();
        assert_eq!(distinct.len(), 400);
    }
}
