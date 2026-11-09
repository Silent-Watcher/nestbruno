//! tsconfig.json parsing via oxc (JSONC support).
//!
//! We parse tsconfig as a TypeScript expression `(...)` to handle comments and
//! trailing commas (JSONC). Only `compilerOptions.baseUrl` and
//! `compilerOptions.paths` are extracted; everything else is ignored.

use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, ObjectExpression, ObjectPropertyKind, PropertyKey};
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::resolve::limits::MAX_SOURCE_BYTES;

/// Parsed tsconfig relevant to module resolution.
#[derive(Debug, Clone, Default)]
pub struct TsConfig {
    /// `compilerOptions.baseUrl` if present.
    pub base_url: Option<String>,
    /// `compilerOptions.paths` mappings: pattern -> list of targets.
    /// Only the first target is used (TS tries each in order).
    pub paths: Vec<PathsEntry>,
}

/// One `paths` entry: pattern -> targets. We only use the first target.
#[derive(Debug, Clone)]
pub struct PathsEntry {
    pub pattern: String,
    pub target: String,
}

impl TsConfig {
    /// Try to load and parse `tsconfig.json` from the project root.
    ///
    /// Returns `None` if the file doesn't exist, is too large, or fails to parse.
    /// Invalid config is treated as absent (safe fallback).
    pub fn load(project_root: &Path) -> Option<Self> {
        let path = project_root.join("tsconfig.json");
        let text = std::fs::read_to_string(&path).ok()?;
        if text.len() > MAX_SOURCE_BYTES {
            return None;
        }
        Self::parse(&text)
    }

    /// Parse tsconfig text (JSONC) into a TsConfig.
    pub fn parse(text: &str) -> Option<Self> {
        // Wrap in parens so the object literal is parsed as an expression.
        // JSONC comments and trailing commas are valid TS syntax.
        let source = format!("({text})");
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &source, SourceType::ts()).parse();

        if parsed.panicked || !parsed.errors.is_empty() {
            return None;
        }

        // The top-level should be an expression statement with a parenthesized object.
        let expr = match &parsed.program.body.first() {
            Some(oxc_ast::ast::Statement::ExpressionStatement(stmt)) => &stmt.expression,
            _ => return None,
        };
        let obj = match expr {
            Expression::ParenthesizedExpression(paren) => &paren.expression,
            _ => return None,
        };
        let obj = match obj {
            Expression::ObjectExpression(obj) => obj,
            _ => return None,
        };

        let mut config = TsConfig::default();
        for prop in &obj.properties {
            match prop {
                ObjectPropertyKind::ObjectProperty(p) => {
                    let key = match &p.key {
                        PropertyKey::StaticIdentifier(id) => id.name.as_str(),
                        PropertyKey::StringLiteral(lit) => lit.value.as_str(),
                        _ => continue,
                    };
                    if key == "compilerOptions"
                        && let Expression::ObjectExpression(co) = &p.value
                    {
                        config.extract_compiler_options(co);
                    }
                }
                _ => continue,
            }
        }
        Some(config)
    }

    fn extract_compiler_options(&mut self, co: &ObjectExpression) {
        for prop in &co.properties {
            match prop {
                ObjectPropertyKind::ObjectProperty(p) => {
                    let key = match &p.key {
                        PropertyKey::StaticIdentifier(id) => id.name.as_str(),
                        PropertyKey::StringLiteral(lit) => lit.value.as_str(),
                        _ => continue,
                    };
                    match key {
                        "baseUrl" => {
                            if let Expression::StringLiteral(lit) = &p.value {
                                self.base_url = Some(lit.value.to_string());
                            }
                        }
                        "paths" => {
                            if let Expression::ObjectExpression(paths) = &p.value {
                                self.extract_paths(paths);
                            }
                        }
                        _ => {}
                    }
                }
                _ => continue,
            }
        }
    }

    fn extract_paths(&mut self, paths: &ObjectExpression) {
        for prop in &paths.properties {
            let (pattern, target) = match prop {
                ObjectPropertyKind::ObjectProperty(p) => {
                    let pattern = match &p.key {
                        PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                        PropertyKey::StringLiteral(lit) => lit.value.to_string(),
                        _ => continue,
                    };
                    let target = match &p.value {
                        Expression::ArrayExpression(arr) => {
                            // Take first string element
                            arr.elements.iter().find_map(|el| match el {
                                oxc_ast::ast::ArrayExpressionElement::StringLiteral(lit) => {
                                    Some(lit.value.to_string())
                                }
                                _ => None,
                            })
                        }
                        _ => None,
                    };
                    (pattern, target)
                }
                _ => continue,
            };
            if let Some(target) = target {
                self.paths.push(PathsEntry { pattern, target });
            }
        }
    }

    /// Resolve a bare specifier (not starting with ./ or ../) using paths and baseUrl.
    ///
    /// Returns the resolved relative path (project-relative) if matched, else None.
    /// The caller is responsible for checking existence and containment.
    pub fn resolve_bare_specifier(&self, specifier: &str, tsconfig_dir: &Path) -> Option<PathBuf> {
        // 1. paths with wildcards: longest prefix match
        let mut best_match: Option<&PathsEntry> = None;
        let mut best_prefix_len = 0;
        for entry in &self.paths {
            if let Some(prefix_len) = match_paths_pattern(&entry.pattern, specifier)
                && prefix_len > best_prefix_len
            {
                best_prefix_len = prefix_len;
                best_match = Some(entry);
            }
        }
        if let Some(entry) = best_match {
            let resolved = apply_paths_mapping(entry, specifier);
            let candidate = tsconfig_dir.join(&resolved);
            return Some(candidate);
        }

        // 2. Exact paths matches (no wildcard)
        for entry in &self.paths {
            if !entry.pattern.contains('*') && entry.pattern == specifier {
                let candidate = tsconfig_dir.join(&entry.target);
                return Some(candidate);
            }
        }

        // 3. baseUrl fallback
        if let Some(base_url) = &self.base_url {
            let base_path = tsconfig_dir.join(base_url);
            let candidate = base_path.join(specifier);
            return Some(candidate);
        }

        None
    }
}

/// Check if `specifier` matches `pattern` (which may contain one `*`).
/// Returns the length of the prefix before `*` if matched, else None.
fn match_paths_pattern(pattern: &str, specifier: &str) -> Option<usize> {
    let star_idx = pattern.find('*');
    let star_idx = match star_idx {
        Some(idx) => idx,
        None => {
            // Exact match already handled elsewhere; but allow for completeness
            return (pattern == specifier).then_some(pattern.len());
        }
    };
    if pattern.matches('*').count() != 1 {
        // TS rejects multiple *; we ignore such patterns
        return None;
    }
    let prefix = &pattern[..star_idx];
    let suffix = &pattern[star_idx + 1..];
    if !specifier.starts_with(prefix) {
        return None;
    }
    if !specifier.ends_with(suffix) {
        return None;
    }
    if specifier.len() <= prefix.len() + suffix.len() {
        // * must match at least one character
        return None;
    }
    Some(prefix.len())
}

/// Apply the paths mapping: replace `*` in target with captured part from specifier.
fn apply_paths_mapping(entry: &PathsEntry, specifier: &str) -> String {
    if let Some(star_idx) = entry.pattern.find('*') {
        let prefix = &entry.pattern[..star_idx];
        let suffix = &entry.pattern[star_idx + 1..];
        let capture = &specifier[prefix.len()..specifier.len() - suffix.len()];
        entry.target.replace('*', capture)
    } else {
        entry.target.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_tsconfig() {
        let text = r#"
{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@users/*": ["src/users/*"],
      "@shared/*": ["src/shared/*"]
    }
  }
}
"#;
        let cfg = TsConfig::parse(text).unwrap();
        assert_eq!(cfg.base_url, Some(".".to_owned()));
        assert_eq!(cfg.paths.len(), 2);
        assert_eq!(cfg.paths[0].pattern, "@users/*");
        assert_eq!(cfg.paths[0].target, "src/users/*");
        assert_eq!(cfg.paths[1].pattern, "@shared/*");
        assert_eq!(cfg.paths[1].target, "src/shared/*");
    }

    #[test]
    fn handles_jsonc_comments_and_trailing_commas() {
        let text = r#"
{
  // comment
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@users/*": ["src/users/*"], // trailing
    },
  },
}
"#;
        let cfg = TsConfig::parse(text).unwrap();
        assert_eq!(cfg.base_url, Some(".".to_owned()));
        assert_eq!(cfg.paths.len(), 1);
    }

    #[test]
    fn ignores_invalid_tsconfig() {
        let text = "not valid";
        assert!(TsConfig::parse(text).is_none());
    }

    #[test]
    fn paths_wildcard_matching() {
        assert_eq!(
            match_paths_pattern("@users/*", "@users/dto/create-user.dto"),
            Some(7)
        );
        assert_eq!(match_paths_pattern("@users/*", "@users/"), None); // * must match >= 1 char
        assert_eq!(match_paths_pattern("@users/*", "@other/dto"), None);
        assert_eq!(match_paths_pattern("*", "anything"), Some(0));
    }

    #[test]
    fn paths_mapping_application() {
        let entry = PathsEntry {
            pattern: "@users/*".into(),
            target: "src/users/*".into(),
        };
        assert_eq!(
            apply_paths_mapping(&entry, "@users/dto/create-user.dto"),
            "src/users/dto/create-user.dto"
        );
        assert_eq!(apply_paths_mapping(&entry, "@users/x"), "src/users/x");
    }
}
