//! Node.js `imports` subpaths: the conservative static subset of
//! `package.json` `imports` that cross-file resolution understands.
//!
//! Supported: top-level `#`-prefixed keys mapping to local string targets or
//! arrays of string targets (the first entry is used, like the tsconfig
//! `paths` policy), with exact or single-`*` wildcard patterns. Targets must
//! start with `./` (package-local); anything else — external packages,
//! nested condition objects, non-string entries — is not resolved and
//! degrades to the conservative unknown shape. `node_modules` is never
//! traversed.

use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, ObjectPropertyKind, PropertyKey};
use oxc_parser::Parser;
use oxc_span::SourceType;

/// The parsed `imports` mappings of one `package.json`, in declaration order.
#[derive(Debug, Clone, Default)]
pub struct PackageImports {
    mappings: Vec<(String, String)>,
}

impl PackageImports {
    /// Parse `package.json` text; `None` when it is absent-shaped (no
    /// `imports` object) or malformed.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() > crate::resolve::limits::MAX_SOURCE_BYTES {
            return None;
        }
        // Wrap in parens so the object literal parses as an expression (the
        // same trick the tsconfig parser uses for JSONC).
        let source = format!("({text})");
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &source, SourceType::ts()).parse();
        if parsed.panicked || !parsed.errors.is_empty() {
            return None;
        }
        let statement = parsed.program.body.first()?;
        let oxc_ast::ast::Statement::ExpressionStatement(expression) = statement else {
            return None;
        };
        // The source was wrapped in parens; unwrap the parenthesized
        // expression the same way the tsconfig parser does.
        let expression = match &expression.expression {
            Expression::ParenthesizedExpression(paren) => &paren.expression,
            expression => expression,
        };
        let Expression::ObjectExpression(object) = expression else {
            return None;
        };

        let imports = object
            .properties
            .iter()
            .filter_map(|property| match property {
                ObjectPropertyKind::ObjectProperty(p) => Some(p),
                _ => None,
            })
            .find(|property| property_name(property) == Some("imports"))?;
        let Expression::ObjectExpression(mappings) = &imports.value else {
            return None;
        };

        let mut parsed_mappings = Vec::new();
        for property in mappings.properties.iter() {
            let Some(property) = (match property {
                ObjectPropertyKind::ObjectProperty(p) => Some(p),
                _ => None,
            }) else {
                continue;
            };
            let Some(key) = property_name(property) else {
                continue;
            };
            if !key.starts_with('#') {
                // Only `imports` keys (which must start with #) are subpaths.
                continue;
            }
            let target = match &property.value {
                Expression::StringLiteral(literal) => Some(literal.value.to_string()),
                // Array targets: the first entry is used.
                Expression::ArrayExpression(array) => match array.elements.first() {
                    Some(oxc_ast::ast::ArrayExpressionElement::StringLiteral(literal)) => {
                        Some(literal.value.to_string())
                    }
                    _ => None,
                },
                // Nested condition objects and other shapes are unsupported.
                _ => None,
            };
            if let Some(target) = target {
                parsed_mappings.push((key.to_owned(), target));
            }
        }
        Some(Self {
            mappings: parsed_mappings,
        })
    }

    /// Map a `#`-prefixed specifier to a project-relative path. The first
    /// matching pattern wins; targets must be package-local (`./...`).
    pub fn resolve(&self, specifier: &str) -> Option<PathBuf> {
        for (pattern, target) in &self.mappings {
            let mapped = match pattern.split_once('*') {
                // A non-matching wildcard pattern is a skip, never an early
                // return: later patterns may match exactly.
                Some((prefix, suffix)) => {
                    let Some(body) = specifier
                        .strip_prefix(prefix)
                        .and_then(|rest| rest.strip_suffix(suffix))
                    else {
                        continue;
                    };
                    target.replacen('*', body, 1)
                }
                None if pattern == specifier => target.clone(),
                None => continue,
            };
            // Local targets only; `..`, absolute and bare-package targets are
            // not resolved (no node_modules traversal).
            let Some(relative) = mapped
                .strip_prefix("./")
                .filter(|relative| !relative.is_empty())
            else {
                continue;
            };
            let path = Path::new(relative);
            if path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                return Some(path.to_path_buf());
            }
        }
        None
    }
}

fn property_name<'a>(property: &oxc_ast::ast::ObjectProperty<'a>) -> Option<&'a str> {
    match &property.key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_and_wildcard_mappings() {
        let text = r##"{
          "name": "app",
          "imports": {
            "#src/*": "./src/*",
            "#config": ["./config/index.ts", "./config/default.ts"]
          }
        }"##;
        let imports = PackageImports::parse(text).unwrap();
        assert_eq!(
            imports.resolve("#src/shared/pager.dto.js"),
            Some(PathBuf::from("src/shared/pager.dto.js"))
        );
        assert_eq!(
            imports.resolve("#config"),
            Some(PathBuf::from("config/index.ts"))
        );
        assert_eq!(imports.resolve("#missing"), None);
    }

    #[test]
    fn external_and_unsafe_targets_are_not_resolved() {
        let text = r##"{
          "imports": {
            "#ext/*": "external-pkg/*",
            "#parent/*": "./../outside/*",
            "#shared": { "node": "./x.ts", "default": "./y.ts" }
          }
        }"##;
        let imports = PackageImports::parse(text).unwrap();
        assert_eq!(imports.resolve("#ext/thing"), None);
        assert_eq!(imports.resolve("#parent/thing"), None);
        assert_eq!(imports.resolve("#shared"), None);
    }

    #[test]
    fn a_file_without_imports_parses_to_none() {
        assert!(PackageImports::parse(r#"{ "name": "app" }"#).is_none());
        assert!(PackageImports::parse("not json").is_none());
    }
}
