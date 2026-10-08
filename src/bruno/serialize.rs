//! Serialization of a [`BrunoRequest`] into `.bru` text.
//!
//! The layout mirrors what Bruno's own serializer writes (`bruno-lang`
//! `jsonToBru`): blocks in the order `meta`, method, `params:query`,
//! `params:path`, `headers`, `body:json`, `docs`; two-space indentation;
//! a blank line between blocks; a single trailing newline; entries written as
//! `key: value`, with an empty value producing `key: ` exactly as Bruno does.
//! Disabled entries are prefixed with `~`. Keys containing `:`, `"`, `{`, `}`
//! or a space are quoted with `\"` escaping.
//!
//! The serializer is deliberately paranoid: it re-validates every string and
//! fails instead of emitting anything that Bruno could read differently from
//! what was intended (line breaks, `'''` openers, unrepresentable keys). A
//! failure produces no text at all, so a partially valid file can never exist.

use std::fmt::Write as _;

use super::model::{Body, BrunoRequest, is_hazardous};
use crate::route::HttpMethod;

/// Why a request could not be serialized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field} cannot be written to a .bru file: {reason}")]
pub struct SerializeError {
    pub field: &'static str,
    pub reason: &'static str,
}

/// Serialize `request` to the exact bytes of its `.bru` file.
pub fn serialize(request: &BrunoRequest) -> Result<String, SerializeError> {
    let mut out = String::new();

    // meta
    single_line("name", &request.name)?;
    out.push_str("meta {\n");
    let _ = writeln!(out, "  name: {}", request.name);
    out.push_str("  type: http\n");
    let _ = writeln!(out, "  seq: {}", request.seq);
    out.push_str("}\n\n");

    // method block + url
    single_line("url", &request.url)?;
    let _ = writeln!(out, "{} {{", method_block(request.method)?);
    let _ = writeln!(out, "  url: {}", request.url);
    out.push_str(match request.body {
        Body::None => "  body: none\n",
        Body::Json(_) => "  body: json\n",
    });
    out.push_str("  auth: inherit\n}\n\n");

    // params:query (all disabled)
    if !request.query_params.is_empty() {
        out.push_str("params:query {\n");
        for name in &request.query_params {
            let _ = writeln!(out, "  ~{}: ", key("query parameter", name)?);
        }
        out.push_str("}\n\n");
    }

    // params:path
    if !request.path_params.is_empty() {
        out.push_str("params:path {\n");
        for name in &request.path_params {
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(SerializeError {
                    field: "path parameter",
                    reason: "only letters, digits and '_' are allowed",
                });
            }
            let _ = writeln!(out, "  {name}: ");
        }
        out.push_str("}\n\n");
    }

    // headers (all disabled)
    if !request.headers.is_empty() {
        out.push_str("headers {\n");
        for name in &request.headers {
            let _ = writeln!(out, "  ~{}: ", key("header", name)?);
        }
        out.push_str("}\n\n");
    }

    // body
    if let Body::Json(json) = &request.body {
        out.push_str("body:json {\n");
        for line in json.split('\n') {
            if line.chars().any(is_hazardous) {
                return Err(SerializeError {
                    field: "body",
                    reason: "contains a control character",
                });
            }
            let _ = writeln!(out, "  {line}");
        }
        out.push_str("}\n\n");
    }

    // docs
    if !request.docs.is_empty() {
        out.push_str("docs {\n");
        for line in &request.docs {
            single_line("docs", line)?;
            let _ = writeln!(out, "  {line}");
        }
        out.push_str("}\n\n");
    }

    // Bruno removes the final blank line.
    out.truncate(out.trim_end_matches('\n').len());
    out.push('\n');
    Ok(out)
}

/// Bruno's name of the method block. `All` has none.
fn method_block(method: HttpMethod) -> Result<&'static str, SerializeError> {
    Ok(match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        HttpMethod::Head => "head",
        HttpMethod::Options => "options",
        HttpMethod::All => {
            return Err(SerializeError {
                field: "method",
                reason: "ALL has no Bruno request type",
            });
        }
    })
}

/// A value must stay on one line and must not open a multi-line block.
fn single_line(field: &'static str, value: &str) -> Result<(), SerializeError> {
    if value.chars().any(is_hazardous) {
        return Err(SerializeError {
            field,
            reason: "contains a control character or line break",
        });
    }
    if value.starts_with("'''") {
        return Err(SerializeError {
            field,
            reason: "starts with ''', which Bruno reads as a multi-line value",
        });
    }
    Ok(())
}

/// A dictionary key, quoted when Bruno requires it.
fn key(field: &'static str, name: &str) -> Result<String, SerializeError> {
    let fail = |reason| SerializeError { field, reason };
    single_line(field, name)?;
    if name.is_empty() || name.trim() != name {
        return Err(fail("empty or padded with whitespace"));
    }
    if name.starts_with('~') || name.starts_with('@') {
        return Err(fail("starts with '~' or '@'"));
    }
    if name.contains('\\') {
        return Err(fail("contains a backslash"));
    }
    if name
        .chars()
        .any(|c| matches!(c, ':' | '"' | '{' | '}' | ' '))
    {
        Ok(format!("\"{}\"", name.replace('"', "\\\"")))
    } else {
        Ok(name.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: HttpMethod) -> BrunoRequest {
        BrunoRequest {
            name: "GET /users".into(),
            seq: 1,
            method,
            url: "{{baseUrl}}/users".into(),
            path_params: vec![],
            query_params: vec![],
            headers: vec![],
            body: Body::None,
            docs: vec![],
        }
    }

    #[test]
    fn minimal_request_matches_bruno_layout() {
        assert_eq!(
            serialize(&request(HttpMethod::Get)).unwrap(),
            "meta {\n  name: GET /users\n  type: http\n  seq: 1\n}\n\nget {\n  url: {{baseUrl}}/users\n  body: none\n  auth: inherit\n}\n"
        );
    }

    #[test]
    fn every_method_uses_its_lowercase_block_name() {
        for (method, block) in [
            (HttpMethod::Get, "get"),
            (HttpMethod::Post, "post"),
            (HttpMethod::Put, "put"),
            (HttpMethod::Patch, "patch"),
            (HttpMethod::Delete, "delete"),
            (HttpMethod::Head, "head"),
            (HttpMethod::Options, "options"),
        ] {
            let text = serialize(&request(method)).unwrap();
            assert!(
                text.contains(&format!("\n{block} {{\n  url: ")),
                "{block}: {text}"
            );
        }
    }

    #[test]
    fn all_cannot_be_serialized() {
        assert_eq!(
            serialize(&request(HttpMethod::All)).unwrap_err().field,
            "method"
        );
    }

    #[test]
    fn full_request_has_blocks_in_bruno_order() {
        let mut r = request(HttpMethod::Post);
        r.name = "POST /users/:id".into();
        r.seq = 3;
        r.url = "{{baseUrl}}/users/:id".into();
        r.path_params = vec!["id".into()];
        r.query_params = vec![
            "page".into(),
            "filter[name]".into(),
            "with space".into(),
            "q\"x".into(),
        ];
        r.headers = vec!["authorization".into(), "x:y".into()];
        r.body = Body::Json("{\n  \"name\": null\n}".into());
        r.docs = vec!["NestJS: UsersController.create".into()];
        assert_eq!(
            serialize(&r).unwrap(),
            "\
meta {
  name: POST /users/:id
  type: http
  seq: 3
}

post {
  url: {{baseUrl}}/users/:id
  body: json
  auth: inherit
}

params:query {
  ~page: 
  ~filter[name]: 
  ~\"with space\": 
  ~\"q\\\"x\": 
}

params:path {
  id: 
}

headers {
  ~authorization: 
  ~\"x:y\": 
}

body:json {
  {
    \"name\": null
  }
}

docs {
  NestJS: UsersController.create
}
"
        );
    }

    #[test]
    fn output_is_deterministic_and_ends_with_one_newline() {
        let mut r = request(HttpMethod::Get);
        r.docs = vec!["a".into()];
        let first = serialize(&r).unwrap();
        assert_eq!(first, serialize(&r).unwrap());
        assert!(first.ends_with("}\n") && !first.ends_with("\n\n"));
        assert!(!first.contains('\r'));
    }

    #[test]
    fn hazardous_values_are_refused_everywhere() {
        let bad = ["a\nb", "a\rb", "a\u{0}b", "a\u{2028}b", "'''x"];
        for value in bad {
            let mut r = request(HttpMethod::Get);
            r.name = value.into();
            assert!(serialize(&r).is_err(), "name {value:?}");

            let mut r = request(HttpMethod::Get);
            r.url = value.into();
            assert!(serialize(&r).is_err(), "url {value:?}");

            let mut r = request(HttpMethod::Get);
            r.docs = vec![value.into()];
            assert!(serialize(&r).is_err(), "docs {value:?}");
        }
        for value in ["a\nb", "\u{0}", "~x", "@x", "a\\b", "", " x", "'''"] {
            let mut r = request(HttpMethod::Get);
            r.query_params = vec![value.into()];
            assert!(serialize(&r).is_err(), "query {value:?}");
            let mut r = request(HttpMethod::Get);
            r.headers = vec![value.into()];
            assert!(serialize(&r).is_err(), "header {value:?}");
        }
        let mut r = request(HttpMethod::Get);
        r.path_params = vec!["a b".into()];
        assert!(serialize(&r).is_err());
        let mut r = request(HttpMethod::Post);
        r.body = Body::Json("{\n\u{0}\n}".into());
        assert!(serialize(&r).is_err());
    }

    #[test]
    fn body_lines_are_always_indented_so_a_brace_cannot_end_the_block() {
        let mut r = request(HttpMethod::Post);
        r.body = Body::Json("{\n}\n}".into());
        let text = serialize(&r).unwrap();
        assert!(text.contains("body:json {\n  {\n  }\n  }\n}\n"), "{text}");
    }

    #[test]
    fn a_failed_serialization_yields_no_text() {
        let mut r = request(HttpMethod::Get);
        r.docs = vec!["ok".into(), "bad\n".into()];
        assert!(serialize(&r).is_err());
    }
}
