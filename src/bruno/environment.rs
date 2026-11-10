//! Bruno environment file generation (Phase 16).
//!
//! # Discovery: what is referenced, never what is secret
//!
//! The discovery scans the **generated** requests for `{{name}}` template
//! references — the same Bruno syntax the requests already use (today only
//! the configured `base_url` emits them). Nothing is read from the analyzed
//! application's sources: no `.env` files, no runtime values, no inference
//! from `process.env` — those live in code NestBruno does not analyze, and
//! even if a reference were visible it would carry no value. Generated
//! environment files therefore contain **empty placeholders by
//! construction** and can never leak a secret.
//!
//! # Ownership
//!
//! The Bruno environment format has no `docs` block, so the generated-file
//! stamp cannot attach to it: an environment file is user-owned the moment
//! it exists. The writer therefore only ever creates files that are absent
//! (like `bruno.json`) and reports what already exists; it never rewrites,
//! never deletes, and never merges.

use std::collections::{BTreeMap, BTreeSet};

use crate::bruno::GeneratedRequest;
use crate::diag::{Diagnostic, Severity};

/// One discovered environment variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentVariable {
    /// The name inside `{{...}}` in the generated requests.
    pub name: String,
    /// `true` while the only discovery source is the request URL template:
    /// every request needs the variable to resolve.
    pub required: bool,
    /// Always `None`: values are never inferred, guessed or copied.
    pub default: Option<String>,
    /// The generated request files that reference the variable
    /// (`folder/file`), sorted, without duplicates.
    pub usages: Vec<String>,
}

/// The result of the discovery scan.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EnvironmentRegistry {
    /// Unique variables, sorted by name (deterministic output).
    pub variables: Vec<EnvironmentVariable>,
    /// Unsupported references, reported with names only.
    pub diagnostics: Vec<Diagnostic>,
}

/// Collect the environment variables the generated requests reference.
///
/// Deduplication is by variable identity: the same name referenced by many
/// requests (or many times in one request) produces exactly one definition
/// with aggregated usages.
pub fn collect_environment_variables(requests: &[GeneratedRequest]) -> EnvironmentRegistry {
    let mut variables: BTreeMap<String, EnvironmentVariable> = BTreeMap::new();
    let mut unsupported: BTreeSet<String> = BTreeSet::new();

    for request in requests {
        let usage = format!("{}/{}", request.folder, request.file_name);
        for reference in referenced_variables(&request.content) {
            if is_valid_variable_name(&reference) {
                let name = reference.clone();
                match variables.entry(name) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(EnvironmentVariable {
                            name: reference,
                            required: true,
                            default: None,
                            usages: vec![usage.clone()],
                        });
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        let variable = entry.get_mut();
                        if !variable.usages.contains(&usage) {
                            variable.usages.push(usage.clone());
                        }
                    }
                }
            } else {
                unsupported.insert(reference);
            }
        }
    }

    // Usages are deterministic anyway (requests arrive in a deterministic
    // order), but sort for independence from that guarantee.
    for variable in variables.values_mut() {
        variable.usages.sort();
    }
    let diagnostics = unsupported
        .into_iter()
        .map(|reference| {
            Diagnostic::new(
                Severity::Warning,
                None,
                format!(
                    "skipping unsupported environment variable reference {{{{{reference}}}}}: \
                     only plain names (letters, digits, '_', '-', '.') can be generated"
                ),
            )
        })
        .collect();
    EnvironmentRegistry {
        variables: variables.into_values().collect(),
        diagnostics,
    }
}

/// The `{{...}}` references of one generated file, in order of occurrence.
/// An unclosed `{{` ends the scan (it is not a reference).
fn referenced_variables(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let inner = after[..end].trim();
        if !inner.is_empty() {
            out.push(inner.to_owned());
        }
        rest = &after[end + 2..];
    }
    out
}

/// Whether `name` can be written as a Bruno environment variable.
///
/// Plain and portable: letters, digits, `_`, `-`, `.` — no whitespace, no
/// braces, no quotes, and not a code expression like `process.env.X` (that
/// is a source-language reference, not a Bruno variable).
pub fn is_valid_variable_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['.', '-'])
        && !name.ends_with('.')
        && !name.starts_with("process.")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The exact content of a generated environment file: `vars { name: }` for
/// every variable, sorted by name, with empty placeholders. Fails instead of
/// writing a name that is not a safe Bruno variable.
pub fn environment_content(variables: &[EnvironmentVariable]) -> Result<String, String> {
    let mut out = String::from("vars {\n");
    for variable in variables {
        if !is_valid_variable_name(&variable.name) {
            return Err(format!(
                "variable name {:?} is not a safe Bruno variable name; no environment file was written",
                variable.name
            ));
        }
        out.push_str("  ");
        out.push_str(&variable.name);
        out.push_str(": \n");
    }
    out.push_str("}\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bruno::identity::RouteIdentity;
    use crate::bruno::stamp;
    use std::path::PathBuf;

    fn request(folder: &str, file: &str, content: &str) -> GeneratedRequest {
        GeneratedRequest {
            folder: folder.to_owned(),
            file_name: file.to_owned(),
            identity: RouteIdentity {
                controller: crate::bruno::identity::ControllerIdentity {
                    name: "C".to_owned(),
                    file: PathBuf::from("src/c.ts"),
                },
                method: crate::route::HttpMethod::Get,
                path: crate::route::RoutePath::Static("/x".to_owned()),
                handler: "h".to_owned(),
            },
            content: content.to_owned(),
            stamp: stamp::Stamp {
                id: 0,
                key: 0,
                anchor: 0,
                hash: 0,
            },
        }
    }

    #[test]
    fn discovers_the_base_url_variable_once() {
        let content = "get {\n  url: {{baseUrl}}/users\n}\n";
        let registry = collect_environment_variables(&[request("Users", "find-all.bru", content)]);
        assert_eq!(registry.variables.len(), 1);
        let variable = &registry.variables[0];
        assert_eq!(variable.name, "baseUrl");
        assert!(variable.required);
        assert_eq!(variable.default, None);
        assert_eq!(variable.usages, ["Users/find-all.bru"]);
        assert!(registry.diagnostics.is_empty());
    }

    #[test]
    fn deduplicates_by_identity_across_requests_and_files() {
        let registry = collect_environment_variables(&[
            request(
                "Users",
                "find-all.bru",
                "url: {{API_BASE_URL}}/users\nurl2: {{API_BASE_URL}}/again",
            ),
            request("Auth", "login.bru", "url: {{API_BASE_URL}}/login"),
        ]);
        assert_eq!(registry.variables.len(), 1);
        assert_eq!(
            registry.variables[0].usages,
            ["Auth/login.bru", "Users/find-all.bru"]
        );
    }

    #[test]
    fn a_dotted_name_is_a_distinct_variable() {
        let registry = collect_environment_variables(&[request(
            "U",
            "a.bru",
            "url: {{baseUrl}}/x and {{api.base}}/y",
        )]);
        let names: Vec<&str> = registry.variables.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["api.base", "baseUrl"], "sorted by name");
    }

    #[test]
    fn unsupported_references_become_warnings_with_names_only() {
        let registry = collect_environment_variables(&[
            request("U", "a.bru", "url: {{ process.env.API_URL }}/x"),
            request("U", "b.bru", "url: {{a b}}/x and {{ process.env.API_URL }}"),
            request("U", "c.bru", "url: {{unclosed"),
        ]);
        assert!(registry.variables.is_empty());
        assert_eq!(
            registry.diagnostics.len(),
            2,
            "one per unique unsupported name"
        );
        let messages: Vec<&str> = registry
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        assert!(messages[0].contains("a b"), "{messages:?}");
        assert!(messages[1].contains("process.env.API_URL"), "{messages:?}");
        assert!(!messages[0].contains("http"), "no values are reported");
    }

    #[test]
    fn valid_names_reject_expressions_and_paths() {
        for name in [
            "",
            " ",
            "a b",
            "a{b",
            "a\"b",
            "a:b",
            ".hidden",
            "trailing.",
            "-dash",
            "process.env.X",
            "process.env",
        ] {
            assert!(!is_valid_variable_name(name), "{name:?} must be rejected");
        }
        for name in ["baseUrl", "API_BASE_URL", "api.base", "tenant-id", "v1_2"] {
            assert!(is_valid_variable_name(name), "{name:?} must be accepted");
        }
    }

    #[test]
    fn environment_files_are_deterministic_sorted_placeholders() {
        let registry =
            collect_environment_variables(&[request("U", "a.bru", "url: {{API_KEY}}/{{baseUrl}}")]);
        let content = environment_content(&registry.variables).unwrap();
        assert_eq!(content, "vars {\n  API_KEY: \n  baseUrl: \n}\n");
        let again = environment_content(&registry.variables).unwrap();
        assert_eq!(content, again);
    }

    #[test]
    fn an_unsafe_name_fails_serialization_instead_of_being_written() {
        let variables = vec![EnvironmentVariable {
            name: "bad name".to_owned(),
            required: true,
            default: None,
            usages: vec![],
        }];
        assert!(environment_content(&variables).is_err());
    }
}
