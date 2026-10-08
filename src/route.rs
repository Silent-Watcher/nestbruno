//! The route intermediate representation (IR).
//!
//! This is the stable boundary between NestJS source analysis and everything
//! downstream. It describes *what the source declares*, in NestJS terms
//! (`:id` parameters, original method names), and knows nothing about Bruno.
//!
//! # Ordering
//!
//! Everything is kept in **source order**: controllers in the order their
//! classes appear in the file, routes in the order their methods appear in the
//! class, parameters by position then decorator order. Across files the order
//! is the (sorted) discovery order.
//!
//! # Path convention
//!
//! A static path is *normalized*: it starts with `/`, has no empty segments
//! and no trailing `/`; the root route is exactly `/`. See [`compose_path`].

use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// A position in a source file. `line` and `column` are 1-based; the column
/// counts Unicode scalar values. Derived from AST spans, never from text search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    /// Path relative to the project root, shared by all locations of one file.
    pub file: Arc<Path>,
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.column)
    }
}

/// The HTTP method of a route. `All` is NestJS's `@All()` (any method).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    All,
}

impl HttpMethod {
    /// Map a NestJS decorator name (`"Get"`, `"Post"`, ...) to a method.
    pub fn from_decorator(name: &str) -> Option<Self> {
        Some(match name {
            "Get" => Self::Get,
            "Post" => Self::Post,
            "Put" => Self::Put,
            "Patch" => Self::Patch,
            "Delete" => Self::Delete,
            "Head" => Self::Head,
            "Options" => Self::Options,
            "All" => Self::All,
            _ => return None,
        })
    }

    /// Upper-case method name as used in HTTP (`ALL` for `@All()`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::All => "ALL",
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A path that is either known statically or could not be determined without
/// evaluating code. Unknown values are never guessed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RoutePath {
    /// A normalized path such as `/users/:id`.
    Static(String),
    /// Built from an expression that is not a plain string literal.
    Unresolved,
}

impl fmt::Display for RoutePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static(path) => f.write_str(path),
            Self::Unresolved => f.write_str("<unresolved>"),
        }
    }
}

/// Kind of a supported parameter decorator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterKind {
    Param,
    Query,
    Body,
    Headers,
}

impl ParameterKind {
    /// Map a NestJS decorator name to a parameter kind.
    pub fn from_decorator(name: &str) -> Option<Self> {
        Some(match name {
            "Param" => Self::Param,
            "Query" => Self::Query,
            "Body" => Self::Body,
            "Headers" => Self::Headers,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Param => "Param",
            Self::Query => "Query",
            Self::Body => "Body",
            Self::Headers => "Headers",
        }
    }
}

/// The key a parameter decorator selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterName {
    /// `@Param()`: the whole object.
    Unnamed,
    /// `@Param('id')`.
    Named(String),
    /// `@Param(KEY)`: the key is not a string literal.
    Unresolved,
}

/// A decorated handler parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteParameter {
    pub kind: ParameterKind,
    pub name: ParameterName,
    /// Zero-based position among **all** formal parameters of the method
    /// (undecorated ones count).
    pub index: usize,
}

/// One HTTP route: a decorated method of a controller. The owning controller
/// is the [`Controller`] that contains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The TypeScript method name.
    pub method_name: String,
    pub http_method: HttpMethod,
    /// Full path: controller path composed with the route path.
    pub path: RoutePath,
    pub parameters: Vec<RouteParameter>,
    /// Location of the route decorator (e.g. `@Get`).
    pub source: SourceLocation,
}

/// A class decorated with `@Controller`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controller {
    pub name: String,
    /// The controller's own path prefix.
    pub path: RoutePath,
    /// Location of the `@Controller` decorator.
    pub source: SourceLocation,
    pub routes: Vec<Route>,
}

/// Join path fragments into one normalized path.
///
/// Rules (and nothing more, route syntax is preserved verbatim):
/// - fragments are split on `/`; empty segments are dropped, so leading,
///   trailing and repeated slashes disappear;
/// - the result starts with `/`; no fragments or only empty ones give `/`;
/// - segments are otherwise untouched: `:id`, `:id?`, `*`, `.`, `..`, case and
///   regex-like characters are preserved.
///
/// The operation is idempotent and `compose_path(&[a, b])` equals
/// `compose_path(&[&compose_path(&[a]), b])`.
pub fn compose_path(fragments: &[&str]) -> String {
    let mut path = String::new();
    for segment in fragments
        .iter()
        .flat_map(|fragment| fragment.split('/'))
        .filter(|segment| !segment.is_empty())
    {
        path.push('/');
        path.push_str(segment);
    }
    if path.is_empty() {
        path.push('/');
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn composes_the_documented_examples() {
        let cases: [(&[&str], &str); 10] = [
            (&["", ""], "/"),
            (&["users", ""], "/users"),
            (&["", "users"], "/users"),
            (&["users", ":id"], "/users/:id"),
            (&["/users/", "/:id/"], "/users/:id"),
            (&["api/users", "profile"], "/api/users/profile"),
            (&["//users//", "//profile//"], "/users/profile"),
            (&["users", ":id?"], "/users/:id?"),
            (&["users", ":id/posts"], "/users/:id/posts"),
            (&["/"], "/"),
        ];
        for (input, expected) in cases {
            assert_eq!(compose_path(input), expected, "{input:?}");
        }
    }

    #[test]
    fn semantic_route_syntax_is_preserved() {
        assert_eq!(compose_path(&["files", "*"]), "/files/*");
        assert_eq!(compose_path(&["a", "b.json"]), "/a/b.json");
        assert_eq!(compose_path(&["Users", "ID"]), "/Users/ID");
        assert_eq!(compose_path(&["a", ".."]), "/a/..");
        assert_eq!(compose_path(&["ab(c|d)+"]), "/ab(c|d)+");
    }

    #[test]
    fn no_fragments_is_the_root() {
        assert_eq!(compose_path(&[]), "/");
    }

    #[test]
    fn method_and_kind_mappings_cover_exactly_the_supported_names() {
        for (name, method) in [
            ("Get", HttpMethod::Get),
            ("Post", HttpMethod::Post),
            ("Put", HttpMethod::Put),
            ("Patch", HttpMethod::Patch),
            ("Delete", HttpMethod::Delete),
            ("Head", HttpMethod::Head),
            ("Options", HttpMethod::Options),
            ("All", HttpMethod::All),
        ] {
            assert_eq!(HttpMethod::from_decorator(name), Some(method));
            assert_eq!(method.as_str().to_lowercase(), name.to_lowercase());
        }
        for name in ["get", "GET", "MyGet", "Controller", "", "Sse"] {
            assert_eq!(HttpMethod::from_decorator(name), None, "{name}");
        }
        for name in ["Param", "Query", "Body", "Headers"] {
            assert_eq!(ParameterKind::from_decorator(name).unwrap().as_str(), name);
        }
        for name in ["Req", "Res", "Header", "param", "Ip"] {
            assert_eq!(ParameterKind::from_decorator(name), None, "{name}");
        }
    }

    #[test]
    fn display_forms() {
        assert_eq!(RoutePath::Unresolved.to_string(), "<unresolved>");
        assert_eq!(RoutePath::Static("/a".into()).to_string(), "/a");
        assert_eq!(HttpMethod::Options.to_string(), "OPTIONS");
        let loc = SourceLocation {
            file: Arc::from(Path::new("src/a.ts")),
            line: 12,
            column: 3,
        };
        assert_eq!(loc.to_string(), "src/a.ts:12:3");
    }

    proptest! {
        #[test]
        fn result_is_always_normalized(parts in proptest::collection::vec(".*", 0..4)) {
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            let path = compose_path(&refs);
            prop_assert!(path.starts_with('/'));
            prop_assert!(!path.contains("//"));
            prop_assert!(path == "/" || !path.ends_with('/'));
        }

        #[test]
        fn composition_is_idempotent(parts in proptest::collection::vec(".*", 0..4)) {
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            let once = compose_path(&refs);
            prop_assert_eq!(compose_path(&[&once]), once);
        }

        #[test]
        fn composition_can_be_split_anywhere(a in ".*", b in ".*") {
            let joined = compose_path(&[&a, &b]);
            prop_assert_eq!(compose_path(&[&compose_path(&[&a]), &b]), joined.clone());
            prop_assert_eq!(compose_path(&[&compose_path(&[&a]), &compose_path(&[&b])]), joined);
        }

        #[test]
        fn non_empty_segments_are_never_altered(
            segs in proptest::collection::vec("[^/]+", 1..5)
        ) {
            let refs: Vec<&str> = segs.iter().map(String::as_str).collect();
            prop_assert_eq!(compose_path(&refs), format!("/{}", segs.join("/")));
        }
    }
}
