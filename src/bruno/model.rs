//! The Bruno request model and the mapping from the route IR to it.
//!
//! Only what the generator needs is modelled. Every string that originates in
//! the (untrusted) NestJS source is validated here, because it ends up in a
//! file that Bruno interprets: values can reference variables (`{{...}}`, which
//! may read environment variables), and a stray newline could open new blocks
//! such as an executable `script:pre-request`.
//!
//! # Mapping
//!
//! | IR | Bruno |
//! | --- | --- |
//! | HTTP method | method block (`get {`, `post {`, ...); `@All` has no Bruno equivalent and is not generated |
//! | path `/users/:id` | `url: {base_url}/users/:id` plus `params:path { id: }` |
//! | optional segment `:id?` | `:id` (a `?` would start a query string); noted in `docs` |
//! | `@Query('page')` | **disabled** `~page:` in `params:query` (visible, editable, not sent) |
//! | `@Headers('x')` | **disabled** `~x:` in `headers` |
//! | `@Body(...)` | `body: json`; `{}` or one `null` field per named `@Body('f')` |
//! | unnamed `@Query()`/`@Headers()`/`@Body()` | a note in `docs`; no fields are invented |
//!
//! Path parameters come from the *URL* (`:name` segments), not from `@Param`
//! decorators, because that is what Bruno itself reads.

use crate::route::{
    BodyShape, Controller, HttpMethod, ParameterKind, ParameterName, Route, RoutePath, compose_path,
};

/// A request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    None,
    /// Raw JSON text, possibly several lines.
    Json(String),
}

/// A request as Bruno stores it. Parameter values are always empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrunoRequest {
    pub name: String,
    /// Position among the requests of the folder (1-based).
    pub seq: usize,
    pub method: HttpMethod,
    pub url: String,
    pub path_params: Vec<String>,
    /// Written disabled.
    pub query_params: Vec<String>,
    /// Written disabled.
    pub headers: Vec<String>,
    pub body: Body,
    /// Lines of the `docs` block.
    pub docs: Vec<String>,
}

/// A request plus the non-fatal problems met while building it.
#[derive(Debug)]
pub struct Built {
    pub request: BrunoRequest,
    /// Parameters that were left out, each with the reason.
    pub warnings: Vec<String>,
}

/// Build the request for `route`. `Err` carries the reason the route cannot be
/// represented at all.
pub fn build_request(
    base_url: &str,
    controller: &Controller,
    route: &Route,
    seq: usize,
) -> Result<Built, String> {
    if route.http_method == HttpMethod::All {
        return Err("@All matches every HTTP method and Bruno has no such request type".to_owned());
    }
    let RoutePath::Static(path) = &route.path else {
        return Err("its path could not be determined statically".to_owned());
    };

    let mut warnings = Vec::new();
    let (path, path_params, optional) = url_path(path)?;
    let url = format!("{}{path}", base_url.trim_end_matches('/'));

    let mut query_params = Vec::new();
    let mut headers = Vec::new();
    let mut body_fields = Vec::new();
    let mut docs = vec![
        format!(
            "NestJS: {}.{}",
            display(&controller.name),
            display(&route.method_name)
        ),
        format!("Source: {}", display(&route.source.file.to_string_lossy())),
    ];
    if !optional.is_empty() {
        docs.push(format!(
            "Optional path parameter(s): {}",
            optional.join(", ")
        ));
    }

    for parameter in &route.parameters {
        let label = format!("@{}", parameter.kind.as_str());
        // Record the statically known type in docs when available.
        if let Some(type_name) = &parameter.type_name {
            let name_desc = match &parameter.name {
                ParameterName::Unnamed => "()".to_owned(),
                ParameterName::Named(n) => format!("({})", display(n)),
                ParameterName::Unresolved => "(...)".to_owned(),
            };
            docs.push(format!("{label}{name_desc}: type {}", display(type_name)));
        }
        match (&parameter.kind, &parameter.name) {
            (ParameterKind::Param, _) => {}
            (kind, ParameterName::Unnamed) => {
                if *kind == ParameterKind::Body {
                    // Only say "fields not inferred" when the body shape is
                    // actually unknown; DTO fields are already in the body.
                    if matches!(route.body, BodyShape::Unknown) {
                        docs.push(format!("{label}(): whole object, fields not inferred"));
                    }
                } else {
                    docs.push(format!("{label}(): whole object, fields not inferred"));
                }
            }
            (_, ParameterName::Unresolved) => {
                docs.push(format!(
                    "{label}(...): name could not be determined statically"
                ));
            }
            (kind, ParameterName::Named(name)) => match check_key(name) {
                Ok(()) => match kind {
                    ParameterKind::Query => push_unique(&mut query_params, name, false),
                    ParameterKind::Headers => push_unique(&mut headers, name, true),
                    ParameterKind::Body => push_unique(&mut body_fields, name, false),
                    ParameterKind::Param => {}
                },
                Err(reason) => warnings.push(format!(
                    "{label} parameter {:?} was left out: {reason}",
                    display(name)
                )),
            },
        }
    }

    // Combine statically known body fields: DTO properties from the route IR
    // plus any named @Body('field') parameters. Named body parameters select a
    // single field, so they take precedence and are merged with the DTO fields.
    // If route.body is None but there are @Body parameters (test fixtures, etc.),
    // fall back to Unknown semantics.
    let has_body_param = route
        .parameters
        .iter()
        .any(|p| p.kind == ParameterKind::Body);
    let effective_body = if has_body_param && route.body == BodyShape::None {
        BodyShape::Unknown
    } else {
        route.body.clone()
    };
    let body = match &effective_body {
        BodyShape::None => Body::None,
        BodyShape::Unknown => {
            let fields = body_fields;
            Body::Json(json_skeleton(&fields))
        }
        BodyShape::Fields(dto_fields) => {
            let mut fields = dto_fields.clone();
            for field in &body_fields {
                push_unique(&mut fields, field, false);
            }
            Body::Json(json_skeleton(&fields))
        }
    };

    Ok(Built {
        request: BrunoRequest {
            name: format!("{} {}", route.http_method, display(&path)),
            seq,
            method: route.http_method,
            url,
            path_params,
            query_params,
            headers,
            body,
            docs,
        },
        warnings,
    })
}

/// Validate and transform a route path for use in a URL.
///
/// Returns the URL path, the path parameter names (from `:name` segments) and
/// the names of optional (`:name?`) parameters.
fn url_path(path: &str) -> Result<(String, Vec<String>, Vec<String>), String> {
    check_untrusted(path).map_err(|reason| format!("its path {reason}"))?;

    let mut params = Vec::new();
    let mut optional = Vec::new();
    let mut segments = Vec::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        let Some(name) = segment.strip_prefix(':') else {
            segments.push(segment.to_owned());
            continue;
        };
        let (name, is_optional) = match name.strip_suffix('?') {
            Some(stripped) => (stripped, true),
            None => (name, false),
        };
        let simple =
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if simple {
            if is_optional {
                optional.push(name.to_owned());
            }
            if !params.iter().any(|p| p == name) {
                params.push(name.to_owned());
            }
            segments.push(format!(":{name}"));
        } else {
            // Not a plain `:name` parameter (modifiers, regex, ...): keep the
            // text verbatim and let the user deal with it in Bruno.
            segments.push(segment.to_owned());
        }
    }
    let path = compose_path(&segments.iter().map(String::as_str).collect::<Vec<_>>());

    // `?` and `#` would end the path in a URL; whitespace would split it.
    if let Some(bad) = path
        .chars()
        .find(|c| matches!(c, '?' | '#') || c.is_whitespace())
    {
        return Err(format!(
            "its path contains {bad:?}, which cannot be written into a URL safely"
        ));
    }
    Ok((path, params, optional))
}

/// Reject text that must never reach a `.bru` file as a functional value.
pub fn check_untrusted(text: &str) -> Result<(), String> {
    if let Some(c) = text.chars().find(|c| is_hazardous(*c)) {
        return Err(format!(
            "contains the control character U+{:04X}",
            u32::from(c)
        ));
    }
    if text.contains("{{") {
        return Err("contains '{{', which Bruno would interpret as a variable".to_owned());
    }
    Ok(())
}

/// Control characters and Unicode line separators.
pub fn is_hazardous(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')
}

/// A parameter name must be representable as a Bruno key.
fn check_key(name: &str) -> Result<(), String> {
    check_untrusted(name)?;
    if name.is_empty() || name.trim() != name {
        return Err(
            "empty names and names with leading/trailing whitespace are not representable"
                .to_owned(),
        );
    }
    if name.starts_with('~') {
        return Err("names starting with '~' would be read as disabled entries".to_owned());
    }
    if name.starts_with('@') {
        return Err("names starting with '@' would be read as annotations".to_owned());
    }
    if name.contains('\\') {
        return Err("names containing a backslash cannot be quoted in Bruno files".to_owned());
    }
    Ok(())
}

fn push_unique(list: &mut Vec<String>, name: &str, ignore_case: bool) {
    let exists = list.iter().any(|existing| {
        if ignore_case {
            existing.eq_ignore_ascii_case(name)
        } else {
            existing == name
        }
    });
    if !exists {
        list.push(name.to_owned());
    }
}

/// `{}` for no known fields, otherwise one `null` per field. `null` is the
/// honest "type unknown" value; no DTO is inspected.
fn json_skeleton(fields: &[String]) -> String {
    if fields.is_empty() {
        return "{}".to_owned();
    }
    let lines: Vec<String> = fields
        .iter()
        .map(|field| format!("  {}: null", json_string(field)))
        .collect();
    format!("{{\n{}\n}}", lines.join(",\n"))
}

/// A JSON string literal. Control characters are escaped, so the result never
/// contains a raw line break.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if is_hazardous(c) => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Text for human-facing fields (names, docs): characters that could break
/// the file format or markdown rendering are replaced by `_`.
fn display(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() || " _-./:#()$@,'*?+=[]".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::route::{BodyShape, RouteParameter, SourceLocation};

    fn location(line: u32) -> SourceLocation {
        SourceLocation {
            file: Arc::from(Path::new("src/users/users.controller.ts")),
            line,
            column: 3,
        }
    }

    fn controller() -> Controller {
        Controller {
            name: "UsersController".into(),
            path: RoutePath::Static("/users".into()),
            source: location(1),
            routes: Vec::new(),
        }
    }

    fn route(method: HttpMethod, path: &str, parameters: Vec<RouteParameter>) -> Route {
        Route {
            method_name: "handler".into(),
            http_method: method,
            path: RoutePath::Static(path.into()),
            parameters,
            body: BodyShape::None,
            source: location(5),
        }
    }

    fn param(kind: ParameterKind, name: ParameterName, index: usize) -> RouteParameter {
        RouteParameter {
            kind,
            name,
            index,
            type_name: None,
        }
    }

    fn param_with_type(
        kind: ParameterKind,
        name: ParameterName,
        index: usize,
        type_name: &str,
    ) -> RouteParameter {
        RouteParameter {
            kind,
            name,
            index,
            type_name: Some(type_name.to_owned()),
        }
    }

    fn named(name: &str) -> ParameterName {
        ParameterName::Named(name.into())
    }

    fn build(route: &Route) -> Result<Built, String> {
        build_request("{{baseUrl}}", &controller(), route, 1)
    }

    #[test]
    fn simple_get_request() {
        let built = build(&route(HttpMethod::Get, "/users", vec![])).unwrap();
        let r = built.request;
        assert_eq!(r.name, "GET /users");
        assert_eq!(r.url, "{{baseUrl}}/users");
        assert!(r.path_params.is_empty() && r.query_params.is_empty() && r.headers.is_empty());
        assert_eq!(r.body, Body::None);
        assert_eq!(r.docs[0], "NestJS: UsersController.handler");
        assert_eq!(r.docs[1], "Source: src/users/users.controller.ts");
        assert!(built.warnings.is_empty());
    }

    #[test]
    fn root_path_and_base_url_joining() {
        let r = build_request(
            "http://localhost:3000/",
            &controller(),
            &route(HttpMethod::Get, "/", vec![]),
            1,
        )
        .unwrap()
        .request;
        assert_eq!(r.url, "http://localhost:3000/");
        let r = build_request("", &controller(), &route(HttpMethod::Get, "/a", vec![]), 1)
            .unwrap()
            .request;
        assert_eq!(r.url, "/a");
    }

    #[test]
    fn path_parameters_are_taken_from_the_url() {
        let r = build(&route(HttpMethod::Get, "/users/:id/posts/:postId", vec![]))
            .unwrap()
            .request;
        assert_eq!(r.url, "{{baseUrl}}/users/:id/posts/:postId");
        assert_eq!(r.path_params, ["id", "postId"]);
    }

    #[test]
    fn similar_routes_keep_distinct_parameter_names() {
        let a = build(&route(HttpMethod::Get, "/users/:id", vec![]))
            .unwrap()
            .request;
        let b = build(&route(HttpMethod::Get, "/users/:userId", vec![]))
            .unwrap()
            .request;
        assert_ne!(a.url, b.url);
        assert_ne!(a.path_params, b.path_params);
    }

    #[test]
    fn repeated_path_parameters_are_listed_once() {
        let r = build(&route(HttpMethod::Get, "/a/:id/b/:id", vec![]))
            .unwrap()
            .request;
        assert_eq!(r.path_params, ["id"]);
    }

    #[test]
    fn optional_parameters_lose_the_question_mark_and_are_noted() {
        let r = build(&route(HttpMethod::Get, "/users/:id?", vec![]))
            .unwrap()
            .request;
        assert_eq!(r.url, "{{baseUrl}}/users/:id");
        assert_eq!(r.path_params, ["id"]);
        assert!(r.docs.iter().any(|d| d == "Optional path parameter(s): id"));
    }

    #[test]
    fn complex_segments_are_kept_verbatim_without_a_params_entry() {
        let r = build(&route(HttpMethod::Get, "/files/:name.json/*", vec![]))
            .unwrap()
            .request;
        assert_eq!(r.url, "{{baseUrl}}/files/:name.json/*");
        assert!(r.path_params.is_empty());
    }

    #[test]
    fn query_and_header_parameters_are_listed_in_parameter_order() {
        use ParameterKind::{Headers, Query};
        let r = build(&route(
            HttpMethod::Get,
            "/users",
            vec![
                param(Query, named("page"), 0),
                param(Headers, named("Authorization"), 1),
                param(Query, named("limit"), 2),
                param(Query, named("page"), 3),
                param(Headers, named("authorization"), 4),
            ],
        ))
        .unwrap()
        .request;
        assert_eq!(r.query_params, ["page", "limit"]);
        assert_eq!(r.headers, ["Authorization"]);
        assert_eq!(
            r.url, "{{baseUrl}}/users",
            "disabled params stay out of the URL"
        );
    }

    #[test]
    fn unnamed_and_unresolved_objects_are_recorded_in_docs_without_inventing_fields() {
        use ParameterKind::{Headers, Query};
        let r = build(&route(
            HttpMethod::Get,
            "/users",
            vec![
                param(Query, ParameterName::Unnamed, 0),
                param(Headers, ParameterName::Unresolved, 1),
            ],
        ))
        .unwrap()
        .request;
        assert!(r.query_params.is_empty() && r.headers.is_empty());
        assert!(
            r.docs
                .iter()
                .any(|d| d.starts_with("@Query(): whole object"))
        );
        assert!(
            r.docs
                .iter()
                .any(|d| d.starts_with("@Headers(...): name could not"))
        );
    }

    #[test]
    fn body_forms() {
        use ParameterKind::Body as BodyParam;
        let json = |parameters| match build(&route(HttpMethod::Post, "/users", parameters))
            .unwrap()
            .request
            .body
        {
            Body::Json(text) => text,
            Body::None => "<none>".to_owned(),
        };
        assert_eq!(json(vec![]), "<none>");
        assert_eq!(
            json(vec![param(BodyParam, ParameterName::Unnamed, 0)]),
            "{}"
        );
        assert_eq!(
            json(vec![param(BodyParam, ParameterName::Unresolved, 0)]),
            "{}"
        );
        assert_eq!(
            json(vec![
                param(BodyParam, named("name"), 0),
                param(BodyParam, named("email"), 1),
                param(BodyParam, named("name"), 2)
            ]),
            "{\n  \"name\": null,\n  \"email\": null\n}"
        );
    }

    #[test]
    fn dto_body_shape_produces_fields() {
        // A route with no parameters: BodyShape drives the body directly.
        let no_params = route(HttpMethod::Post, "/users", vec![]);

        // BodyShape::None, no body params -> None
        let route = Route {
            body: BodyShape::None,
            ..no_params.clone()
        };
        let built = build_request("{{baseUrl}}", &controller(), &route, 1).unwrap();
        assert_eq!(built.request.body, Body::None);

        // BodyShape::Unknown, no body params -> {}
        let route = Route {
            body: BodyShape::Unknown,
            ..no_params.clone()
        };
        let built = build_request("{{baseUrl}}", &controller(), &route, 1).unwrap();
        assert_eq!(built.request.body, Body::Json("{}".into()));

        // BodyShape::Fields -> fields in skeleton
        let route = Route {
            body: BodyShape::Fields(vec!["name".into(), "email".into()]),
            ..no_params
        };
        let built = build_request("{{baseUrl}}", &controller(), &route, 1).unwrap();
        assert_eq!(
            built.request.body,
            Body::Json("{\n  \"name\": null,\n  \"email\": null\n}".into())
        );
    }

    #[test]
    fn named_body_parameters_are_merged_with_dto_fields() {
        use ParameterKind::Body as BodyParam;
        let route = route(
            HttpMethod::Post,
            "/users",
            vec![
                param_with_type(BodyParam, named("name"), 0, "string"),
                param(BodyParam, ParameterName::Unnamed, 1),
            ],
        );
        let built = build_request("{{baseUrl}}", &controller(), &route, 1).unwrap();
        assert_eq!(
            built.request.body,
            Body::Json("{\n  \"name\": null\n}".into())
        );
    }

    #[test]
    fn parameter_type_is_recorded_in_docs() {
        let route = route(
            HttpMethod::Get,
            "/users/:id",
            vec![param_with_type(
                ParameterKind::Param,
                named("id"),
                0,
                "string",
            )],
        );
        let built = build_request("{{baseUrl}}", &controller(), &route, 1).unwrap();
        assert!(
            built
                .request
                .docs
                .iter()
                .any(|d| d == "@Param(id): type string")
        );
    }

    #[test]
    fn query_parameter_type_is_recorded_in_docs() {
        let route = route(
            HttpMethod::Get,
            "/users",
            vec![param_with_type(
                ParameterKind::Query,
                named("page"),
                0,
                "number",
            )],
        );
        let built = build_request("{{baseUrl}}", &controller(), &route, 1).unwrap();
        assert!(
            built
                .request
                .docs
                .iter()
                .any(|d| d == "@Query(page): type number")
        );
    }

    #[test]
    fn path_decorators_do_not_create_query_or_body_entries() {
        let r = build(&route(
            HttpMethod::Get,
            "/users/:id",
            vec![param(ParameterKind::Param, named("id"), 0)],
        ))
        .unwrap()
        .request;
        assert!(r.query_params.is_empty() && r.body == Body::None);
    }

    #[test]
    fn all_is_not_generated() {
        let err = build(&route(HttpMethod::All, "/x", vec![])).unwrap_err();
        assert!(err.contains("@All"));
    }

    #[test]
    fn every_other_method_is_accepted() {
        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
            HttpMethod::Head,
            HttpMethod::Options,
        ] {
            assert_eq!(
                build(&route(method, "/x", vec![])).unwrap().request.method,
                method
            );
        }
    }

    #[test]
    fn unresolved_paths_are_not_generated() {
        let mut r = route(HttpMethod::Get, "/x", vec![]);
        r.path = RoutePath::Unresolved;
        assert!(build(&r).unwrap_err().contains("statically"));
    }

    // ---- security: untrusted text must never reach the file as syntax

    #[test]
    fn paths_with_control_characters_are_rejected() {
        for path in [
            "/a\n}\n\nscript:pre-request {\n  require('child_process')\n}\n",
            "/a\rb",
            "/a\u{0}b",
            "/a\u{2028}b",
            "/a\tb",
            "/a\u{85}b",
        ] {
            assert!(
                build(&route(HttpMethod::Get, path, vec![])).is_err(),
                "{path:?}"
            );
        }
    }

    #[test]
    fn paths_with_variable_interpolation_are_rejected() {
        let err = build(&route(HttpMethod::Get, "/{{process.env.SECRET}}", vec![])).unwrap_err();
        assert!(err.contains("{{"));
    }

    #[test]
    fn paths_that_would_corrupt_the_url_are_rejected() {
        for path in ["/a?b=1", "/a#frag", "/a b", "/:id?x"] {
            assert!(
                build(&route(HttpMethod::Get, path, vec![])).is_err(),
                "{path:?}"
            );
        }
    }

    #[test]
    fn unrepresentable_parameter_names_are_left_out_with_a_warning() {
        use ParameterKind::{Body as BodyParam, Headers, Query};
        let evil = [
            "a\nb",
            "{{process.env.X}}",
            "~off",
            "@ann",
            "back\\slash",
            " lead",
            "",
            "tab\tx",
        ];
        for name in evil {
            for kind in [Query, Headers, BodyParam] {
                let built = build(&route(
                    HttpMethod::Post,
                    "/x",
                    vec![param(kind, named(name), 0)],
                ))
                .unwrap();
                assert!(built.request.query_params.is_empty(), "{name:?}");
                assert!(built.request.headers.is_empty(), "{name:?}");
                assert_eq!(
                    built.request.body.clone(),
                    if kind == BodyParam {
                        Body::Json("{}".into())
                    } else {
                        Body::None
                    },
                    "{name:?}"
                );
                assert_eq!(built.warnings.len(), 1, "{name:?}");
            }
        }
    }

    #[test]
    fn ordinary_unusual_names_are_kept() {
        use ParameterKind::Query;
        let r = build(&route(
            HttpMethod::Get,
            "/x",
            vec![
                param(Query, named("filter[name]"), 0),
                param(Query, named("page.size"), 1),
                param(Query, named("with space"), 2),
                param(Query, named("quote\"d"), 3),
                param(Query, named("ünï"), 4),
            ],
        ))
        .unwrap()
        .request;
        assert_eq!(r.query_params.len(), 5);
    }

    #[test]
    fn json_strings_are_escaped_and_single_line() {
        assert_eq!(json_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(json_string("a\nb"), "\"a\\u000ab\"");
        let skeleton = json_skeleton(&["quote\"d".to_owned(), "x".to_owned()]);
        assert_eq!(skeleton, "{\n  \"quote\\\"d\": null,\n  \"x\": null\n}");
    }

    #[test]
    fn display_text_is_neutralized() {
        assert_eq!(display("Users<script>"), "Users_script_");
        assert_eq!(display("a\nb"), "a_b");
        assert_eq!(display("src/کاربران/a.ts"), "src/کاربران/a.ts");
        assert_eq!(display("`x`{{y}}"), "_x___y__");
    }

    #[test]
    fn names_and_docs_cannot_carry_control_characters() {
        let mut r = route(HttpMethod::Get, "/ok", vec![]);
        r.method_name = "bad\nname".into();
        let built = build(&r).unwrap().request;
        assert!(!built.docs.iter().any(|line| line.contains('\n')));
    }
}
