//! Static analysis of the NestJS application bootstrap: the global prefix
//! and URI-versioning configuration.
//!
//! ```text
//! bootstrap file (main.ts) ─▶ oxc AST ─▶ setGlobalPrefix / enableVersioning
//!                          └▶ BootstrapConfig (static facts only)
//! ```
//!
//! Only statically provable configuration is extracted: string-literal
//! prefixes, `VersioningType.URI` provenance-checked against a real
//! `@nestjs/common` import, string/array default versions, `VERSION_NEUTRAL`
//! provenance, and statically matchable exclude rules (exact paths and
//! trailing-`*` wildcards, with or without a concrete request method).
//! Anything else — dynamic expressions, other versioning strategies,
//! unmatchable exclude patterns — yields a warning and the conservative
//! "no prefix / no version" behavior, never a guessed URL. Nothing is
//! executed or evaluated.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{CallExpression, Expression, ObjectPropertyKind, PropertyKey, Statement};
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::diag::{Diagnostic, Severity};
use crate::route::HttpMethod;

/// The statically provable application configuration.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct BootstrapConfig {
    /// `app.setGlobalPrefix('api')` — applied to every route unless an
    /// exclude rule matches. `None` also when excludes made prefixing
    /// unanalyzable (a warning explains why).
    pub global_prefix: Option<String>,
    exclude_rules: Vec<ExcludeRule>,
    /// URI-versioning configuration (only `VersioningType.URI` is rendered
    /// into URLs; other strategies are identified and reported).
    pub versioning: Option<VersioningConfig>,
    /// Warnings about static-analysis boundaries met while reading the
    /// bootstrap (unsupported strategies, unmatchable excludes, dynamic
    /// arguments).
    pub diagnostics: Vec<Diagnostic>,
}

impl BootstrapConfig {
    /// Whether the global prefix applies to a route with this composed
    /// controller/method path and HTTP method.
    pub fn prefix_applies_to(&self, path: &str, method: HttpMethod) -> bool {
        self.global_prefix.is_some()
            && !self
                .exclude_rules
                .iter()
                .any(|rule| rule.matches(path, method))
    }

    /// The URL version segments a route gets under this configuration, given
    /// the controller- and method-level annotations (NestJS precedence:
    /// method overrides controller; `VERSION_NEUTRAL` at any level removes
    /// the segment; otherwise the application default). Multiple declared
    /// versions yield one segment each — one route variant per version.
    pub fn version_segments(
        &self,
        controller_version: &VersionAnnotation,
        method_version: &VersionAnnotation,
    ) -> Vec<String> {
        let declared: &[String] = match (method_version, controller_version) {
            (VersionAnnotation::Neutral, _) | (_, VersionAnnotation::Neutral) => return vec![],
            (VersionAnnotation::Versions(versions), _) if !versions.is_empty() => versions,
            (_, VersionAnnotation::Versions(versions)) if !versions.is_empty() => versions,
            _ => {
                let Some(versioning) = self.versioning.as_ref() else {
                    return vec![];
                };
                if versioning.default_neutral || versioning.default_versions.is_empty() {
                    return vec![];
                }
                &versioning.default_versions
            }
        };
        declared
            .iter()
            .map(|version| format!("v{version}"))
            .collect()
    }
}

/// The statically provable part of `enableVersioning`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct VersioningConfig {
    pub kind: VersioningKind,
    /// `defaultVersion: '1'` / `['1', '2']`. Empty means no application
    /// default (routes without their own version get none).
    pub default_versions: Vec<String>,
    /// The application default is `VERSION_NEUTRAL` (no version segment).
    pub default_neutral: bool,
}

/// The versioning strategy kind. Only [`VersioningKind::Uri`] is rendered
/// into URL prefixes.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum VersioningKind {
    Uri,
    Header,
    MediaType,
    /// A `type` argument that is not a statically recognizable
    /// `VersioningType` member.
    Unknown,
}

/// The `version` annotation of a controller or a route method.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub enum VersionAnnotation {
    /// No annotation at this level.
    #[default]
    Unset,
    /// `@Version(VERSION_NEUTRAL)` (or `version: VERSION_NEUTRAL`).
    Neutral,
    /// `@Version('2')` / `version: ['1', '2']` — static strings only.
    Versions(Vec<String>),
}

/// One exclude rule of `setGlobalPrefix('api', { exclude: [...] })`.
#[derive(Debug, PartialEq, Eq, Clone)]
enum ExcludeRule {
    /// `'health'` — exact path, every method.
    Exact(String),
    /// `'health*'` — trailing wildcard: the route path must start with the
    /// prefix (the only statically matchable pattern shape).
    Prefix(String),
    /// `{ path: 'health', method: RequestMethod.GET }` — path pattern plus a
    /// concrete method (`None` when the method is not statically
    /// recognizable, in which case the path pattern matches any method).
    Method {
        pattern: ExcludePattern,
        method: Option<HttpMethod>,
    },
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum ExcludePattern {
    Exact(String),
    Prefix(String),
}

impl ExcludeRule {
    fn matches(&self, route_path: &str, method: HttpMethod) -> bool {
        // Exclude patterns are written without a leading slash; route paths
        // always have one. Normalize before comparing.
        let path = route_path.trim_start_matches('/');
        fn pattern_matches(pattern: &ExcludePattern, path: &str) -> bool {
            match pattern {
                ExcludePattern::Exact(exact) => exact == path,
                ExcludePattern::Prefix(prefix) => path.starts_with(prefix.as_str()),
            }
        }
        match self {
            ExcludeRule::Exact(exact) => exact == path,
            ExcludeRule::Prefix(prefix) => path.starts_with(prefix.as_str()),
            ExcludeRule::Method {
                pattern,
                method: rule_method,
            } => {
                rule_method.is_none_or(|rule| rule == method || rule == HttpMethod::All)
                    && pattern_matches(pattern, path)
            }
        }
    }
}

/// Conventional bootstrap entry points, checked in order. The search is
/// bounded to these candidates plus an optional configured path — never a
/// repository scan.
pub const CONVENTIONAL_BOOTSTRAP_FILES: &[&str] = &["src/main.ts", "main.ts"];

/// The bootstrap file the run uses: the configured one, or the first
/// conventional candidate that exists under the project root.
pub fn find_bootstrap_file(project_root: &Path, configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(configured) = configured {
        return std::fs::symlink_metadata(project_root.join(configured))
            .is_ok()
            .then(|| configured.to_path_buf());
    }
    CONVENTIONAL_BOOTSTRAP_FILES
        .iter()
        .map(PathBuf::from)
        .find(|candidate| std::fs::symlink_metadata(project_root.join(candidate)).is_ok())
}

/// Local names bound to `@nestjs/common` imports of a program
/// (imported name -> local name; namespace imports as `"*"`).
pub fn program_common_locals<'a>(
    program: &'a oxc_ast::ast::Program<'_>,
) -> HashMap<&'a str, &'a str> {
    let mut locals = HashMap::new();
    for statement in &program.body {
        if let Some(imported) = import_locals(statement, "@nestjs/common") {
            for (imported, local) in imported {
                locals.insert(imported, local);
            }
        }
    }
    locals
}

/// Analyze the bootstrap source text for the static application
/// configuration. `source_path` names the file in diagnostics.
pub fn analyze_bootstrap(source: &str, source_path: &Path) -> BootstrapConfig {
    let mut config = BootstrapConfig::default();
    if source.len() > crate::nest::extract::MAX_SOURCE_BYTES {
        config.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            Some(source_path.to_path_buf()),
            "the bootstrap file exceeds the source size cap; global prefix and versioning are not applied",
        ));
        return config;
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if parsed.panicked || !parsed.errors.is_empty() {
        config.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            Some(source_path.to_path_buf()),
            "the bootstrap file cannot be parsed; global prefix and versioning are not applied",
        ));
        return config;
    }

    // Provenance for the @nestjs/common symbols this analysis recognizes:
    // imported name -> local name.
    let nestjs_common_locals = program_common_locals(&parsed.program);

    const MAX_DEPTH: u32 = 32;
    for statement in &parsed.program.body {
        walk_statement(
            statement,
            MAX_DEPTH,
            &mut |call| match member_property(call) {
                Some("setGlobalPrefix") => {
                    analyze_set_global_prefix(call, &mut config, source_path)
                }
                Some("enableVersioning") => {
                    analyze_enable_versioning(call, &nestjs_common_locals, &mut config, source_path)
                }
                _ => {}
            },
        );
    }
    config
}

/// The member property name of `something.<name>(...)` calls, if any.
fn member_property<'a>(call: &'a CallExpression<'a>) -> Option<&'a str> {
    match &call.callee {
        Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
        _ => None,
    }
}

/// Local names bound by an import from `specifier` (imported name -> local
/// name), including namespace imports as `"*" -> local`.
fn import_locals<'a>(
    statement: &'a Statement<'a>,
    specifier: &str,
) -> Option<Vec<(&'a str, &'a str)>> {
    let oxc_ast::ast::Statement::ImportDeclaration(declaration) = statement else {
        return None;
    };
    if declaration.source.value.as_str() != specifier {
        return None;
    }
    let specifiers = declaration.specifiers.as_ref()?;
    Some(
        specifiers
            .iter()
            .filter_map(|spec| match spec {
                oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(spec) => {
                    let imported = match &spec.imported {
                        oxc_ast::ast::ModuleExportName::IdentifierName(id) => id.name.as_str(),
                        oxc_ast::ast::ModuleExportName::IdentifierReference(id) => id.name.as_str(),
                        oxc_ast::ast::ModuleExportName::StringLiteral(lit) => lit.value.as_str(),
                    };
                    Some((imported, spec.local.name.as_str()))
                }
                oxc_ast::ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(spec) => {
                    Some(("*", spec.local.name.as_str()))
                }
                oxc_ast::ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => None,
            })
            .collect(),
    )
}

/// Visit every call expression reachable in a statement, descending into
/// function and arrow bodies (the bootstrap's `setGlobalPrefix` typically
/// lives inside `async function bootstrap() { ... }`). Depth-bounded;
/// bootstrap files are shallow.
fn walk_statement<'a>(
    statement: &'a Statement<'a>,
    depth: u32,
    visit: &mut dyn FnMut(&CallExpression<'a>),
) {
    if depth == 0 {
        return;
    }
    match statement {
        Statement::ExpressionStatement(expression) => {
            walk_expression(&expression.expression, depth, visit)
        }
        Statement::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                if let Some(init) = &declarator.init {
                    walk_expression(init, depth, visit);
                }
            }
        }
        Statement::ReturnStatement(returned) => {
            if let Some(argument) = &returned.argument {
                walk_expression(argument, depth, visit);
            }
        }
        Statement::FunctionDeclaration(function) => {
            if let Some(body) = &function.body {
                for statement in &body.statements {
                    walk_statement(statement, depth - 1, visit);
                }
            }
        }
        Statement::BlockStatement(block) => {
            for statement in &block.body {
                walk_statement(statement, depth - 1, visit);
            }
        }
        Statement::IfStatement(if_statement) => {
            walk_statement(&if_statement.consequent, depth - 1, visit);
            if let Some(alternate) = &if_statement.alternate {
                walk_statement(alternate, depth - 1, visit);
            }
        }
        _ => {}
    }
}

fn walk_expression<'a>(
    expression: &'a Expression<'a>,
    depth: u32,
    visit: &mut dyn FnMut(&CallExpression<'a>),
) {
    if depth == 0 {
        return;
    }
    match expression {
        Expression::CallExpression(call) => {
            visit(call);
            walk_expression(&call.callee, depth, visit);
            for argument in &call.arguments {
                if let Some(argument) = argument.as_expression() {
                    walk_expression(argument, depth, visit);
                }
            }
        }
        Expression::StaticMemberExpression(member) => walk_expression(&member.object, depth, visit),
        Expression::ParenthesizedExpression(inner) => {
            walk_expression(&inner.expression, depth, visit)
        }
        Expression::AwaitExpression(awaited) => walk_expression(&awaited.argument, depth, visit),
        Expression::SequenceExpression(sequence) => {
            for expression in &sequence.expressions {
                walk_expression(expression, depth, visit);
            }
        }
        Expression::ArrowFunctionExpression(arrow) => {
            for statement in &arrow.body.statements {
                walk_statement(statement, depth - 1, visit);
            }
        }
        Expression::FunctionExpression(function) => {
            if let Some(body) = &function.body {
                for statement in &body.statements {
                    walk_statement(statement, depth - 1, visit);
                }
            }
        }
        _ => {}
    }
}

fn analyze_set_global_prefix(
    call: &CallExpression<'_>,
    config: &mut BootstrapConfig,
    source_path: &Path,
) {
    let Some(argument) = call
        .arguments
        .first()
        .and_then(|argument| argument.as_expression())
    else {
        return;
    };
    let prefix = match string_literal(argument) {
        Some(prefix) => prefix,
        None => {
            config.diagnostics.push(Diagnostic::new(
                Severity::Warning,
                Some(source_path.to_path_buf()),
                "setGlobalPrefix argument is not a static string; no global prefix is applied",
            ));
            return;
        }
    };
    if prefix.is_empty() {
        return;
    }
    if config.global_prefix.is_some() {
        config.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            Some(source_path.to_path_buf()),
            "setGlobalPrefix is called more than once; the last static prefix wins",
        ));
    }
    config.global_prefix = Some(prefix);

    // Exclude rules: only statically matchable patterns keep prefixing
    // enabled; anything else disables the prefix conservatively.
    let Some(options) = call
        .arguments
        .get(1)
        .and_then(|argument| argument.as_expression())
    else {
        return;
    };
    let Expression::ObjectExpression(object) = options else {
        return;
    };
    let mut excludes: Vec<ExcludeRule> = Vec::new();
    let mut supported = true;
    for property in object.properties.iter() {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            continue;
        };
        if property_name(property) != Some("exclude") {
            continue;
        }
        let Expression::ArrayExpression(array) = &property.value else {
            supported = false;
            break;
        };
        for element in &array.elements {
            let Some(element) = element.as_expression() else {
                supported = false;
                continue;
            };
            match element {
                Expression::StringLiteral(literal) => match classify_pattern(&literal.value) {
                    Some(rule) => excludes.push(rule),
                    None => supported = false,
                },
                Expression::ObjectExpression(rule) => {
                    let mut path: Option<String> = None;
                    let mut method: Option<HttpMethod> = None;
                    for property in rule.properties.iter() {
                        let ObjectPropertyKind::ObjectProperty(property) = property else {
                            continue;
                        };
                        match property_name(property) {
                            Some("path") => {
                                path = string_literal(&property.value);
                            }
                            Some("method") => {
                                if let Expression::StaticMemberExpression(member) = &property.value
                                {
                                    method = request_method(member.property.name.as_str());
                                }
                            }
                            _ => {}
                        }
                    }
                    match path.as_deref().and_then(classify_pattern) {
                        Some(ExcludeRule::Exact(exact)) => excludes.push(ExcludeRule::Method {
                            pattern: ExcludePattern::Exact(exact),
                            method,
                        }),
                        Some(ExcludeRule::Prefix(prefix)) => excludes.push(ExcludeRule::Method {
                            pattern: ExcludePattern::Prefix(prefix),
                            method,
                        }),
                        Some(ExcludeRule::Method { .. }) | None => supported = false,
                    }
                }
                _ => supported = false,
            }
        }
    }
    if supported {
        config.exclude_rules = excludes;
    } else {
        config.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            Some(source_path.to_path_buf()),
            "setGlobalPrefix exclude rules contain a pattern that cannot be matched statically \
             (only exact paths and trailing `*` wildcards are supported); \
             no global prefix is applied",
        ));
        config.global_prefix = None;
    }
}

fn request_method(name: &str) -> Option<HttpMethod> {
    Some(match name {
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

/// `exact` and `trailing-*` patterns are statically matchable; anything else
/// (leading `*`, parameters, `?`, empty) is not.
fn classify_pattern(value: &str) -> Option<ExcludeRule> {
    if value.is_empty() {
        return None;
    }
    if value == "*" {
        // Matches everything: statically provable.
        return Some(ExcludeRule::Prefix(String::new()));
    }
    if let Some(prefix) = value.strip_suffix('*') {
        if prefix.contains('*') || prefix.contains(':') || prefix.contains('?') {
            return None;
        }
        return Some(ExcludeRule::Prefix(prefix.to_owned()));
    }
    if value.contains('*') || value.contains(':') || value.contains('?') {
        return None;
    }
    Some(ExcludeRule::Exact(value.to_owned()))
}

fn analyze_enable_versioning(
    call: &CallExpression<'_>,
    nestjs_common_locals: &HashMap<&str, &str>,
    config: &mut BootstrapConfig,
    source_path: &Path,
) {
    let Some(options) = call
        .arguments
        .first()
        .and_then(|argument| argument.as_expression())
    else {
        return;
    };
    let Expression::ObjectExpression(object) = options else {
        return;
    };
    let mut kind = VersioningKind::Unknown;
    let mut default_versions: Vec<String> = Vec::new();
    let mut default_neutral = false;
    for property in object.properties.iter() {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            continue;
        };
        match property_name(property) {
            Some("type") => {
                kind = versioning_kind(&property.value, nestjs_common_locals);
            }
            Some("defaultVersion") => match &property.value {
                Expression::StringLiteral(literal) => {
                    default_versions.push(literal.value.to_string());
                }
                Expression::ArrayExpression(array) => {
                    for element in &array.elements {
                        match element.as_expression().and_then(|e| string_literal(e)) {
                            Some(version) => default_versions.push(version),
                            None => {
                                if is_version_neutral(
                                    element.as_expression().unwrap_or(&property.value),
                                    nestjs_common_locals,
                                ) {
                                    default_neutral = true;
                                }
                            }
                        }
                    }
                }
                value => {
                    // VERSION_NEUTRAL or a dynamic expression.
                    if is_version_neutral(value, nestjs_common_locals) {
                        default_neutral = true;
                    } else {
                        config.diagnostics.push(Diagnostic::new(
                            Severity::Warning,
                            Some(source_path.to_path_buf()),
                            "enableVersioning defaultVersion is not a static string, array, or \
                             VERSION_NEUTRAL; no application default version is applied",
                        ));
                    }
                }
            },
            _ => {}
        }
    }
    if kind != VersioningKind::Uri {
        config.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            Some(source_path.to_path_buf()),
            match kind {
                VersioningKind::Header => {
                    "header-based versioning does not appear in URLs; no version prefix is added"
                }
                VersioningKind::MediaType => {
                    "media-type versioning does not appear in URLs; no version prefix is added"
                }
                _ => {
                    "enableVersioning type is not a statically recognizable URI versioning; \
                     no version prefix is added"
                }
            },
        ));
        return;
    }
    if config.versioning.is_some() {
        config.diagnostics.push(Diagnostic::new(
            Severity::Warning,
            Some(source_path.to_path_buf()),
            "enableVersioning is called more than once; the last call wins",
        ));
    }
    config.versioning = Some(VersioningConfig {
        kind,
        default_versions,
        default_neutral,
    });
}

fn versioning_kind(
    value: &Expression<'_>,
    nestjs_common_locals: &HashMap<&str, &str>,
) -> VersioningKind {
    let Expression::StaticMemberExpression(member) = value else {
        return VersioningKind::Unknown;
    };
    let Expression::Identifier(object) = &member.object else {
        return VersioningKind::Unknown;
    };
    let Some(local) = nestjs_common_locals.get("VersioningType") else {
        return VersioningKind::Unknown;
    };
    if object.name.as_str() != *local {
        return VersioningKind::Unknown;
    }
    match member.property.name.as_str() {
        "URI" => VersioningKind::Uri,
        "HEADER" => VersioningKind::Header,
        "MEDIA_TYPE" => VersioningKind::MediaType,
        _ => VersioningKind::Unknown,
    }
}

/// Whether the expression is the `VERSION_NEUTRAL` symbol from
/// `@nestjs/common` (provenance-checked).
fn is_version_neutral(
    expression: &Expression<'_>,
    nestjs_common_locals: &HashMap<&str, &str>,
) -> bool {
    let Expression::Identifier(identifier) = expression else {
        return false;
    };
    nestjs_common_locals
        .get("VERSION_NEUTRAL")
        .is_some_and(|local| *local == identifier.name.as_str())
}

fn string_literal(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::StringLiteral(literal) => Some(literal.value.to_string()),
        _ => None,
    }
}

fn property_name<'a>(property: &oxc_ast::ast::ObjectProperty<'a>) -> Option<&'a str> {
    match &property.key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.as_str()),
        _ => None,
    }
}

/// Provenance-checked `VERSION_NEUTRAL`: `Some(expression)` must be the
/// `VERSION_NEUTRAL` symbol imported from `@nestjs/common`.
pub fn is_version_neutral_expr(
    expression: Option<&Expression<'_>>,
    nestjs_common_locals: &HashMap<&str, &str>,
) -> bool {
    expression.is_some_and(|expression| is_version_neutral(expression, nestjs_common_locals))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn analyze(source: &str) -> BootstrapConfig {
        analyze_bootstrap(source, Path::new("src/main.ts"))
    }

    const BOOTSTRAP: &str = "\
import { NestFactory } from '@nestjs/core';
import { AppModule } from './app.module';

async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api');
  await app.listen(3000);
}
bootstrap();
";

    #[test]
    fn extracts_a_static_global_prefix_from_the_bootstrap_body() {
        let config = analyze(BOOTSTRAP);
        assert_eq!(config.global_prefix.as_deref(), Some("api"));
        assert!(config.prefix_applies_to("/users", HttpMethod::Get));
    }

    #[test]
    fn exclude_rules_remove_the_prefix_for_matching_routes() {
        let config = analyze(
            r#"
import { NestFactory } from '@nestjs/core';
import { RequestMethod, VersioningType } from '@nestjs/common';
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api', { exclude: ['health', { path: 'admin', method: RequestMethod.GET }, 'legacy*'] });
}
bootstrap();
"#,
        );
        assert_eq!(config.global_prefix.as_deref(), Some("api"));
        assert!(config.prefix_applies_to("/users", HttpMethod::Post));
        assert!(!config.prefix_applies_to("/health", HttpMethod::Get));
        assert!(!config.prefix_applies_to("/admin", HttpMethod::Get));
        assert!(
            config.prefix_applies_to("/admin", HttpMethod::Post),
            "the method-restricted exclude does not match POST"
        );
        assert!(!config.prefix_applies_to("/legacy/v1", HttpMethod::Delete));
    }

    #[test]
    fn an_unmatchable_exclude_disables_the_prefix_with_a_warning() {
        let config = analyze(
            r#"
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api', { exclude: ['/he*lth'] });
}
bootstrap();
"#,
        );
        assert_eq!(config.global_prefix, None);
        assert!(
            config
                .diagnostics
                .iter()
                .any(|d| d.message.contains("cannot be matched statically"))
        );
    }

    #[test]
    fn a_dynamic_prefix_is_reported_and_not_applied() {
        let config = analyze(
            r#"
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix(process.env.PREFIX);
}
bootstrap();
"#,
        );
        assert_eq!(config.global_prefix, None);
        assert!(
            config
                .diagnostics
                .iter()
                .any(|d| d.message.contains("not a static string"))
        );
    }

    #[test]
    fn uri_versioning_with_a_default_version() {
        let config = analyze(
            r#"
import { VersioningType } from '@nestjs/common';
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.enableVersioning({ type: VersioningType.URI, defaultVersion: '1' });
}
bootstrap();
"#,
        );
        let versioning = config.versioning.as_ref().expect("URI versioning");
        assert_eq!(versioning.kind, VersioningKind::Uri);
        assert_eq!(versioning.default_versions, ["1"]);
        // Precedence: method overrides controller overrides default.
        let unset = VersionAnnotation::Unset;
        assert_eq!(config.version_segments(&unset, &unset), ["v1"]);
        assert_eq!(
            config.version_segments(&VersionAnnotation::Versions(vec!["2".into()]), &unset),
            ["v2"]
        );
        assert_eq!(
            config.version_segments(&unset, &VersionAnnotation::Versions(vec!["3".into()])),
            ["v3"]
        );
        assert_eq!(
            config.version_segments(&unset, &VersionAnnotation::Neutral),
            Vec::<String>::new()
        );
        // Multiple declared versions: one variant each.
        assert_eq!(
            config.version_segments(
                &VersionAnnotation::Versions(vec!["1".into(), "2".into()]),
                &unset
            ),
            ["v1", "v2"]
        );
    }

    #[test]
    fn version_neutral_default_and_unversioned_types_are_conserved() {
        let neutral = analyze(
            r#"
import { VERSION_NEUTRAL, VersioningType } from '@nestjs/common';
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.enableVersioning({ type: VersioningType.URI, defaultVersion: VERSION_NEUTRAL });
}
bootstrap();
"#,
        );
        assert!(neutral.versioning.as_ref().expect("uri").default_neutral);
        assert!(
            neutral
                .version_segments(&Default::default(), &Default::default())
                .is_empty()
        );

        let header = analyze(
            r#"
import { VersioningType } from '@nestjs/common';
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.enableVersioning({ type: VersioningType.HEADER, header: 'X-Version' });
}
bootstrap();
"#,
        );
        assert!(header.versioning.is_none());
        assert!(
            header
                .diagnostics
                .iter()
                .any(|d| d.message.contains("header-based versioning"))
        );
    }

    #[test]
    fn versioning_provenance_requires_nestjs_common() {
        let config = analyze(
            r#"
import { VersioningType } from 'some-other-lib';
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.enableVersioning({ type: VersioningType.URI, defaultVersion: '1' });
}
bootstrap();
"#,
        );
        assert!(config.versioning.is_none());
        assert!(
            config
                .diagnostics
                .iter()
                .any(|d| d.message.contains("not a statically recognizable"))
        );
    }

    #[test]
    fn finds_the_bootstrap_file_deterministically() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_bootstrap_file(dir.path(), None), None);
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.ts"), "export {}").unwrap();
        assert_eq!(
            find_bootstrap_file(dir.path(), None),
            Some(PathBuf::from("src/main.ts"))
        );
        // A configured path wins and may point anywhere project-relative.
        fs::write(dir.path().join("bootstrap.ts"), "export {}").unwrap();
        assert_eq!(
            find_bootstrap_file(dir.path(), Some(Path::new("bootstrap.ts"))),
            Some(PathBuf::from("bootstrap.ts"))
        );
    }
}
