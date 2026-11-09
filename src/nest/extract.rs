//! Static NestJS semantic analysis of one TypeScript source text.
//!
//! ```text
//! source text ─▶ oxc AST ─▶ semantic walk ─▶ Controller / Route IR
//! ```
//!
//! Nothing is executed or evaluated: decorator arguments are inspected
//! structurally and anything that is not a plain string literal is reported as
//! *unresolved* rather than guessed.
//!
//! # Recognition rules (name based)
//!
//! - A class is a controller when it carries a call-form decorator whose callee
//!   is the identifier `Controller`. The class name is irrelevant.
//! - A method is a route when it carries a call-form decorator named `Get`,
//!   `Post`, `Put`, `Patch`, `Delete`, `Head`, `Options` or `All`.
//! - Parameter decorators `Param`, `Query`, `Body`, `Headers` are recorded.
//! - Decorators are matched by *name* only. Import origin is not verified, and
//!   aliased (`Controller as C`) or namespaced (`common.Controller`) decorators
//!   are not recognized.
//! - Only classes declared at the top level of the file (plain, `export`, or
//!   `export default`) are analyzed.
//!
//! # Static values
//!
//! A path argument is static if it is a string literal or a template literal
//! without substitutions. `@Controller({ path: 'x' })` is also understood.
//! Everything else (identifiers, calls, concatenation, `as const`,
//! parentheses, arrays, spreads) is unresolved and produces a warning.
//!
//! # Parameter types and same-file DTOs
//!
//! A parameter's type annotation is recorded only when it is one plain
//! identifier (a keyword like `string`, or a class name like `CreateUserDto`).
//! Arrays, generics, unions, qualified names and intersections are *not*
//! resolved — never guessed, never evaluated.
//!
//! When a `@Body()` parameter's type names a class declared in the **same
//! file**, its property names (source order, computed names skipped, at most
//! 64 per class) become the generated body skeleton. Classes in other files
//! are invisible by construction: discovery only reads files matching
//! `include`, so a cross-file lookup would need project-wide type resolution,
//! which is explicitly out of scope.
//!
//! # Failure policy
//!
//! A file with any syntax error yields **no** controllers and one `error`
//! diagnostic: partial results from a recovered AST could silently drop routes.

use std::cell::OnceCell;
use std::path::Path;
use std::sync::Arc;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, Class, ClassElement, Declaration, Decorator,
    ExportDefaultDeclarationKind, Expression, MethodDefinition, MethodDefinitionKind,
    MethodDefinitionType, ObjectExpression, ObjectPropertyKind, Program, PropertyKey, Statement,
    TSSignature,
};
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};

use crate::diag::{Diagnostic, Severity};
use crate::nest::ProjectContext;
use crate::nest::app::{BootstrapConfig, VersionAnnotation};
use crate::resolve::limits::MAX_DTO_PROPERTIES;
use crate::resolve::resolve_cross_file_dtos;
use crate::route::{
    BodyShape, Controller, HttpMethod, ParameterKind, ParameterName, Route, RouteParameter,
    RoutePath, SourceLocation, compose_path,
};
use std::collections::HashMap;

/// Everything found in one file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FileAnalysis {
    pub controllers: Vec<Controller>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Largest source text that is analyzed. The parser recurses once per nesting
/// level and every level consumes at least one input byte, so this bound (with
/// [`PARSER_STACK_BYTES`]) bounds stack use. Real controllers are a few KiB.
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;

/// Stack budget per source byte, with ~1.5x margin over the worst case
/// measured for the pinned parser (1394 B/byte in a debug build on deeply
/// nested parentheses; 559 B/byte in release).
const STACK_BYTES_PER_SOURCE_BYTE: usize = 2048;

/// Stack size that makes [`extract_controllers`] safe for any input of at most
/// [`MAX_SOURCE_BYTES`]. Run it on a thread created with this stack size (see
/// [`crate::nest::analyze_files`]); the default 2-8 MiB stack is *not*
/// sufficient for adversarial nesting, and a stack overflow aborts the process.
pub const PARSER_STACK_BYTES: usize = MAX_SOURCE_BYTES * STACK_BYTES_PER_SOURCE_BYTE;

/// Analyze `source`, the text of `file` (a path relative to the project root,
/// used only for locations and diagnostics).
///
/// Must run on a stack of at least [`PARSER_STACK_BYTES`] unless the input is
/// trusted to be shallowly nested.
///
/// When `project` is provided, cross-file DTO resolution is performed for
/// `@Body` parameters whose type was not resolved from same-file DTOs, over
/// the run-scoped resolver context it carries.
pub fn extract_controllers(
    file: &Path,
    source: &str,
    project: Option<&mut ProjectContext<'_>>,
) -> FileAnalysis {
    if source.len() > MAX_SOURCE_BYTES {
        return FileAnalysis {
            controllers: Vec::new(),
            diagnostics: vec![too_large(file, source.len())],
        };
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();

    // Extract same-file DTOs first
    let same_file_dtos_vec = dto_properties(&parsed.program);
    let mut same_file_dtos: HashMap<String, Vec<String>> = HashMap::new();
    for (name, props) in &same_file_dtos_vec {
        same_file_dtos.insert(name.clone(), props.clone());
    }

    let app = project.as_deref().map(|project| project.app);
    let mut extractor = Extractor {
        file: Arc::from(file),
        source,
        lines: OnceCell::new(),
        diagnostics: Vec::new(),
        dtos: same_file_dtos_vec,
        app,
        nestjs_common: crate::nest::app::program_common_locals(&parsed.program),
    };

    if parsed.panicked || !parsed.errors.is_empty() {
        let first = parsed.errors.first();
        let message = first.map_or_else(
            || "the parser gave up".to_owned(),
            |error| error.message.to_string(),
        );
        let offset = first
            .and_then(|error| error.labels.as_ref())
            .and_then(|labels| labels.first())
            .and_then(|label| u32::try_from(label.offset()).ok());
        let mut diagnostic = Diagnostic::new(
            Severity::Error,
            Some(file.to_owned()),
            format!(
                "unable to parse TypeScript ({} syntax error(s)); no routes were extracted from this file. First error: {message}",
                parsed.errors.len().max(1)
            ),
        );
        if let Some(offset) = offset {
            let (line, column) = extractor.position(offset);
            diagnostic = diagnostic.at(line, column);
        }
        extractor.diagnostics.push(diagnostic);
        return extractor.finish(Vec::new());
    }

    let mut controllers = Vec::new();
    for statement in &parsed.program.body {
        if let Some(class) = top_level_class(statement)
            && let Some(controller) = extractor.class(class)
        {
            controllers.push(controller);
        }
    }

    let mut file_analysis = extractor.finish(controllers);

    // Cross-file DTO resolution for routes with Unknown body shape
    if let Some(project) = project {
        // The extractor already parsed this file: seed the run cache with its
        // AST-derived metadata so the resolver never re-reads or re-parses
        // the controller file for its own imports.
        crate::resolve::seed_module_from_program(&mut *project.resolver, file, &parsed.program);
        // For each route with Unknown body and @Body parameters, resolve cross-file DTOs
        for controller in &mut file_analysis.controllers {
            for route in &mut controller.routes {
                // Body resolution needs an unknown body; unnamed
                // @Query()/@Param() object parameters resolve regardless of
                // the body shape.
                let has_unnamed_object_param = route.parameters.iter().any(|parameter| {
                    parameter.fields.is_empty()
                        && parameter.type_name.is_some()
                        && parameter.name == ParameterName::Unnamed
                        && matches!(parameter.kind, ParameterKind::Query | ParameterKind::Param)
                });
                if route.body != BodyShape::Unknown && !has_unnamed_object_param {
                    continue;
                }

                // Collect the parameters whose types resolve cross-file:
                // @Body parameters and unnamed @Query()/@Param() objects.
                let body_params: Vec<RouteParameter> = route
                    .parameters
                    .iter()
                    .filter(|p| {
                        p.kind == ParameterKind::Body
                            || (matches!(p.kind, ParameterKind::Query | ParameterKind::Param)
                                && p.name == ParameterName::Unnamed)
                    })
                    .cloned()
                    .collect();

                if body_params.is_empty() {
                    continue;
                }

                // Resolve cross-file DTOs for this route's body parameters
                let resolutions = resolve_cross_file_dtos(
                    &mut *project.resolver,
                    project.source_prefix,
                    project.config,
                    project.matcher,
                    file,
                    &body_params,
                    &same_file_dtos,
                );

                // Apply resolutions per parameter: the @Body parameter's
                // fields become the body shape; unnamed @Query()/@Param()
                // object parameters carry their fields on the parameter.
                for (param_index, resolution) in resolutions.resolutions {
                    let kind = route
                        .parameters
                        .iter()
                        .find(|parameter| parameter.index == param_index)
                        .map(|parameter| parameter.kind);
                    match (&resolution, kind) {
                        (
                            crate::resolve::DtoResolution::Fields(props),
                            Some(ParameterKind::Body),
                        )
                        | (
                            crate::resolve::DtoResolution::ZodFields(props),
                            Some(ParameterKind::Body),
                        ) => {
                            if !props.is_empty() {
                                route.body = BodyShape::Fields(props.clone());
                            }
                        }
                        (
                            crate::resolve::DtoResolution::Fields(props),
                            Some(ParameterKind::Query),
                        )
                        | (
                            crate::resolve::DtoResolution::Fields(props),
                            Some(ParameterKind::Param),
                        ) => {
                            if let Some(parameter) = route
                                .parameters
                                .iter_mut()
                                .find(|parameter| parameter.index == param_index)
                            {
                                parameter.fields = props.clone();
                            }
                        }
                        (
                            crate::resolve::DtoResolution::Uncertain(_),
                            Some(ParameterKind::Body),
                        )
                        | (crate::resolve::DtoResolution::Uncertain(_), None) => {
                            // A partial property list is not a complete body
                            // shape. Keep the conservative Unknown fallback.
                        }
                        (crate::resolve::DtoResolution::Uncertain(props), _) => {
                            // Query/param uncertain: keep the partial names
                            // (they are disabled metadata, never sent).
                            if let Some(parameter) = route
                                .parameters
                                .iter_mut()
                                .find(|parameter| parameter.index == param_index)
                            {
                                parameter.fields = props.clone();
                            }
                        }
                        _ => {}
                    }
                }

                // Merge diagnostics
                file_analysis.diagnostics.extend(resolutions.diagnostics);
            }
        }
    }

    file_analysis
}

/// Collect the property names of every top-level class **and interface**,
/// keyed by name. Only what the body skeleton needs: names in source order,
/// computed keys skipped, methods and constructors ignored.
fn dto_properties(program: &Program<'_>) -> Vec<(String, Vec<String>)> {
    let mut classes = Vec::new();
    for statement in &program.body {
        let (name, properties) = match statement {
            Statement::ClassDeclaration(class) => {
                let Some(id) = &class.id else { continue };
                let mut properties = Vec::new();
                for element in &class.body.body {
                    if let ClassElement::PropertyDefinition(property) = element
                        && let Some(name) = property.key.static_name()
                    {
                        properties.push(name.into_owned());
                        if properties.len() == MAX_DTO_PROPERTIES {
                            break;
                        }
                    }
                }
                (id.name.to_string(), properties)
            }
            Statement::TSInterfaceDeclaration(interface) => {
                let mut properties = Vec::new();
                for signature in &interface.body.body {
                    if let TSSignature::TSPropertySignature(property) = signature
                        && let Some(name) = property.key.static_name()
                    {
                        properties.push(name.into_owned());
                        if properties.len() == MAX_DTO_PROPERTIES {
                            break;
                        }
                    }
                }
                (interface.id.name.to_string(), properties)
            }
            _ => continue,
        };
        classes.push((name, properties));
    }
    classes
}

/// Diagnostic for a source above [`MAX_SOURCE_BYTES`].
pub fn too_large(file: &Path, bytes: usize) -> Diagnostic {
    Diagnostic::new(
        Severity::Error,
        Some(file.to_owned()),
        format!(
            "file is too large to analyze ({bytes} bytes, limit {MAX_SOURCE_BYTES}); no routes were extracted from it"
        ),
    )
}

/// The class declared by a top-level statement, if any.
fn top_level_class<'a, 'b>(statement: &'b Statement<'a>) -> Option<&'b Class<'a>> {
    match statement {
        Statement::ClassDeclaration(class) => Some(class),
        Statement::ExportNamedDeclaration(export) => match &export.declaration {
            Some(Declaration::ClassDeclaration(class)) => Some(class),
            _ => None,
        },
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            ExportDefaultDeclarationKind::ClassDeclaration(class) => Some(class),
            _ => None,
        },
        _ => None,
    }
}

/// The value of a decorator argument.
enum PathArg {
    Static(String),
    /// Not statically known; the reason is shown to the user.
    Dynamic(&'static str),
}

struct Extractor<'s, 'a> {
    file: Arc<Path>,
    source: &'s str,
    /// Built on first use; most files never need it.
    lines: OnceCell<LineIndex>,
    diagnostics: Vec<Diagnostic>,
    /// Property names of every top-level class of the file, keyed by class
    /// name, for same-file body DTO resolution.
    dtos: Vec<(String, Vec<String>)>,
    /// Statically analyzed bootstrap configuration (global prefix, URI
    /// versioning). `None` without a project context.
    app: Option<&'a BootstrapConfig>,
    /// Local names bound to `@nestjs/common` imports of this file, for
    /// `VERSION_NEUTRAL` provenance.
    nestjs_common: HashMap<&'s str, &'s str>,
}

impl Extractor<'_, '_> {
    fn finish(self, controllers: Vec<Controller>) -> FileAnalysis {
        FileAnalysis {
            controllers,
            diagnostics: self.diagnostics,
        }
    }

    fn position(&self, offset: u32) -> (u32, u32) {
        self.lines
            .get_or_init(|| LineIndex::new(self.source))
            .locate(self.source, offset)
    }

    fn location(&self, span: Span) -> SourceLocation {
        let (line, column) = self.position(span.start);
        SourceLocation {
            file: Arc::clone(&self.file),
            line,
            column,
        }
    }

    fn warn(&mut self, span: Span, message: String) {
        let (line, column) = self.position(span.start);
        self.diagnostics.push(
            Diagnostic::new(Severity::Warning, Some(self.file.to_path_buf()), message)
                .at(line, column),
        );
    }

    /// Analyze one class; `None` when it is not a controller.
    fn class(&mut self, class: &Class<'_>) -> Option<Controller> {
        let (decorator, call) = class.decorators.iter().find_map(|decorator| {
            let (name, call) = decorator_call(decorator)?;
            (name == "Controller").then_some((decorator, call))
        })?;

        let Some(id) = &class.id else {
            self.warn(
                decorator.span,
                "@Controller on an anonymous class is not supported; it is skipped".to_owned(),
            );
            return None;
        };
        let name = id.name.to_string();

        let controller_path = match controller_path_arg(call) {
            PathArg::Static(raw) => Some(raw),
            PathArg::Dynamic(reason) => {
                self.warn(
                    decorator.span,
                    format!(
                        "path of controller {name} cannot be determined statically ({reason}); its routes have unresolved paths"
                    ),
                );
                None
            }
        };

        let controller_version = self.controller_version(call);

        let mut routes = Vec::new();
        for element in &class.body.body {
            if let ClassElement::MethodDefinition(method) = element {
                routes.extend(self.routes(
                    &name,
                    controller_path.as_deref(),
                    &controller_version,
                    method,
                ));
            }
        }

        Some(Controller {
            path: controller_path.map_or(RoutePath::Unresolved, |raw| {
                RoutePath::Static(compose_path(&[&raw]))
            }),
            source: self.location(decorator.span),
            name,
            routes,
        })
    }

    /// The controller-level `version` of the `@Controller` options object,
    /// when statically known.
    fn controller_version(&self, call: &CallExpression<'_>) -> VersionAnnotation {
        let Some(options) = call
            .arguments
            .first()
            .and_then(|argument| argument.as_expression())
        else {
            return VersionAnnotation::Unset;
        };
        let Expression::ObjectExpression(object) = options else {
            return VersionAnnotation::Unset;
        };
        for property in object.properties.iter() {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            if property_key_name(&property.key) != Some("version") {
                continue;
            }
            return self.version_annotation(&property.value);
        }
        VersionAnnotation::Unset
    }

    /// The method-level `@Version(...)` annotation, when statically known.
    fn method_version(&self, method: &MethodDefinition<'_>) -> VersionAnnotation {
        for decorator in &method.decorators {
            let Some((name, call)) = decorator_call(decorator) else {
                continue;
            };
            if name != "Version" {
                continue;
            }
            let Some(argument) = call
                .arguments
                .first()
                .and_then(|argument| argument.as_expression())
            else {
                continue;
            };
            return self.version_annotation(argument);
        }
        VersionAnnotation::Unset
    }

    /// A `version` annotation value: a static string, an array of them, or
    /// the provenance-checked `VERSION_NEUTRAL` symbol. Anything else is
    /// `Unset` (the conservative fallback: the app default applies).
    fn version_annotation(&self, value: &Expression<'_>) -> VersionAnnotation {
        match value {
            Expression::StringLiteral(literal) => {
                VersionAnnotation::Versions(vec![literal.value.to_string()])
            }
            Expression::ArrayExpression(array) => {
                let mut versions = Vec::new();
                let mut neutral = false;
                for element in &array.elements {
                    match element.as_expression().map(|e| string_literal_value(e)) {
                        Some(Some(version)) => versions.push(version),
                        Some(None) => {
                            neutral = neutral
                                || crate::nest::app::is_version_neutral_expr(
                                    element.as_expression(),
                                    &self.nestjs_common,
                                );
                        }
                        None => {}
                    }
                }
                if neutral {
                    VersionAnnotation::Neutral
                } else if versions.is_empty() {
                    VersionAnnotation::Unset
                } else {
                    VersionAnnotation::Versions(versions)
                }
            }
            _ => {
                if crate::nest::app::is_version_neutral_expr(Some(value), &self.nestjs_common) {
                    VersionAnnotation::Neutral
                } else {
                    VersionAnnotation::Unset
                }
            }
        }
    }

    /// Analyze one method; `None` when it is not a route. One route per
    /// declared version variant (NestJS URI versioning).
    fn routes(
        &mut self,
        controller: &str,
        controller_path: Option<&str>,
        controller_version: &VersionAnnotation,
        method: &MethodDefinition<'_>,
    ) -> Vec<Route> {
        if method.kind != MethodDefinitionKind::Method
            || method.r#type != MethodDefinitionType::MethodDefinition
            || method.r#static
        {
            return Vec::new();
        }

        let mut route_decorators = method.decorators.iter().filter_map(|decorator| {
            let (name, call) = decorator_call(decorator)?;
            Some((HttpMethod::from_decorator(name)?, decorator, call))
        });
        let Some((http_method, decorator, call)) = route_decorators.next() else {
            return Vec::new();
        };
        let ignored = route_decorators.count();

        let Some(method_name) = method.key.static_name() else {
            self.warn(
                decorator.span,
                format!("route in {controller} has a computed method name; it is skipped"),
            );
            return Vec::new();
        };
        let method_name = method_name.into_owned();

        if ignored > 0 {
            self.warn(
                decorator.span,
                format!(
                    "{controller}.{method_name} has several route decorators; only the first (@{}) is extracted",
                    decorator_name(decorator).unwrap_or("?")
                ),
            );
        }

        let path = match first_path_arg(call) {
            PathArg::Static(route_path) => controller_path
                .map_or(RoutePath::Unresolved, |prefix| {
                    RoutePath::Static(compose_path(&[prefix, &route_path]))
                }),
            PathArg::Dynamic(reason) => {
                self.warn(
                    decorator.span,
                    format!(
                        "path of {controller}.{method_name} cannot be determined statically ({reason})"
                    ),
                );
                RoutePath::Unresolved
            }
        };

        let mut parameters = Vec::new();
        let mut body_param_type: Option<String> = None;
        for (index, parameter) in method.value.params.items.iter().enumerate() {
            let type_name = extract_type_name(parameter.type_annotation.as_deref());
            for decorator in &parameter.decorators {
                let Some((name, call)) = decorator_call(decorator) else {
                    continue;
                };
                let Some(kind) = ParameterKind::from_decorator(name) else {
                    continue;
                };
                let name = parameter_name(call);
                if name == ParameterName::Unresolved {
                    self.warn(
                        decorator.span,
                        format!(
                            "name of @{} parameter {index} in {controller}.{method_name} is not a string literal; it is recorded as unresolved",
                            kind.as_str()
                        ),
                    );
                }
                if kind == ParameterKind::Body {
                    body_param_type = type_name.clone();
                }
                // Unnamed @Query()/@Param() object parameters: same-file DTO
                // property names (cross-file resolution fills the rest later).
                let fields = if name == ParameterName::Unnamed
                    && matches!(kind, ParameterKind::Query | ParameterKind::Param)
                {
                    type_name
                        .as_deref()
                        .and_then(|type_name| self.dtos.iter().find(|(dto, _)| dto == type_name))
                        .map(|(_, fields)| fields.clone())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                parameters.push(RouteParameter {
                    kind,
                    name,
                    index,
                    type_name: type_name.clone(),
                    fields,
                });
            }
        }

        let body = self.resolve_body_shape(&parameters, body_param_type);

        let build = |method_name: &str, path: RoutePath, _version: Option<String>| Route {
            method_name: method_name.to_owned(),
            http_method,
            path,
            parameters: parameters.clone(),
            body: body.clone(),
            source: self.location(decorator.span),
        };

        // The path is unresolved (dynamic route path, or no controller
        // path): no effective URL to compose.
        if matches!(path, RoutePath::Unresolved) {
            return vec![build(&method_name, path, None)];
        }
        let RoutePath::Static(local_path) = path else {
            return vec![build(&method_name, path, None)];
        };
        // Effective URL: `/{prefix}/v{n}/{controller}/{method}` — the global
        // prefix first, then the version segment. NestJS applies excludes to
        // the path WITHOUT the prefix and the version.
        let prefix_fragment: Vec<&str> = match self.app {
            Some(app) if app.prefix_applies_to(&local_path, http_method) => {
                vec![app.global_prefix.as_deref().unwrap_or_default()]
            }
            _ => vec![],
        };
        let method_version = self.method_version(method);
        let version_segments = self
            .app
            .map(|app| app.version_segments(controller_version, &method_version))
            .unwrap_or_default();
        if version_segments.is_empty() {
            let effective = compose_path(&[&prefix_fragment.join("/"), &local_path]);
            return vec![build(&method_name, RoutePath::Static(effective), None)];
        }
        version_segments
            .into_iter()
            .map(|segment| {
                let effective = compose_path(&[&prefix_fragment.join("/"), &segment, &local_path]);
                build(&method_name, RoutePath::Static(effective), Some(segment))
            })
            .collect()
    }

    /// Resolve the statically known request-body shape for this route.
    fn resolve_body_shape(
        &self,
        parameters: &[RouteParameter],
        body_param_type: Option<String>,
    ) -> BodyShape {
        let has_body = parameters.iter().any(|p| p.kind == ParameterKind::Body);
        if !has_body {
            return BodyShape::None;
        }
        let Some(type_name) = body_param_type else {
            return BodyShape::Unknown;
        };
        for (dto_name, properties) in &self.dtos {
            if dto_name == &type_name && !properties.is_empty() {
                return BodyShape::Fields(properties.clone());
            }
        }
        BodyShape::Unknown
    }
}

/// The string value of a plain string literal expression.
fn string_literal_value(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::StringLiteral(literal) => Some(literal.value.to_string()),
        _ => None,
    }
}

/// The static name of an object property key.
fn property_key_name<'a>(key: &'a PropertyKey<'a>) -> Option<&'a str> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.as_str()),
        _ => None,
    }
}

/// Extract a simple type name from a TypeScript type annotation.
///
/// Returns `Some(name)` only for:
/// - Keyword types: `string`, `number`, `boolean`
/// - A single bare identifier reference (e.g., `CreateUserDto`) with no type arguments
///
/// Returns `None` for arrays, generics, unions, qualified names,
/// parenthesized types, etc.
fn extract_type_name(
    type_annotation: Option<&oxc_ast::ast::TSTypeAnnotation<'_>>,
) -> Option<String> {
    let ts_type = &type_annotation?.type_annotation;
    match ts_type {
        oxc_ast::ast::TSType::TSStringKeyword(_) => Some("string".to_owned()),
        oxc_ast::ast::TSType::TSNumberKeyword(_) => Some("number".to_owned()),
        oxc_ast::ast::TSType::TSBooleanKeyword(_) => Some("boolean".to_owned()),
        oxc_ast::ast::TSType::TSTypeReference(type_ref) => {
            // Only accept a simple identifier with no type arguments
            if type_ref.type_arguments.is_some() {
                return None;
            }
            match &type_ref.type_name {
                oxc_ast::ast::TSTypeName::IdentifierReference(ident) => {
                    Some(ident.name.to_string())
                }
                _ => None, // QualifiedName, ThisExpression, etc.
            }
        }
        _ => None, // All other types: arrays, generics, unions, etc.
    }
}

/// `@Name(...)` as `(Name, call)`. Other decorator shapes yield `None`.
fn decorator_call<'a, 'b>(
    decorator: &'b Decorator<'a>,
) -> Option<(&'b str, &'b CallExpression<'a>)> {
    let Expression::CallExpression(call) = &decorator.expression else {
        return None;
    };
    let Expression::Identifier(callee) = &call.callee else {
        return None;
    };
    Some((callee.name.as_str(), call))
}

fn decorator_name<'b>(decorator: &'b Decorator<'_>) -> Option<&'b str> {
    decorator_call(decorator).map(|(name, _)| name)
}

fn first_argument<'a, 'b>(call: &'b CallExpression<'a>) -> Option<&'b Argument<'a>> {
    call.arguments.first()
}

/// A plain string, or a template literal without substitutions.
fn static_string(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::StringLiteral(literal) => Some(literal.value.to_string()),
        Expression::TemplateLiteral(template) => {
            template.single_quasi().map(|atom| atom.to_string())
        }
        _ => None,
    }
}

fn path_from_expression(expression: &Expression<'_>) -> PathArg {
    if let Some(value) = static_string(expression) {
        return PathArg::Static(value);
    }
    PathArg::Dynamic(match expression {
        Expression::ArrayExpression(_) => "an array of paths is not supported yet",
        Expression::TemplateLiteral(_) => "template literal with substitutions",
        _ => "not a string literal",
    })
}

/// The path given as first argument; no argument means the empty path.
fn first_path_arg(call: &CallExpression<'_>) -> PathArg {
    match first_argument(call) {
        None => PathArg::Static(String::new()),
        Some(argument) => match argument.as_expression() {
            Some(expression) => path_from_expression(expression),
            None => PathArg::Dynamic("spread argument"),
        },
    }
}

/// `@Controller()`, `@Controller('x')` or `@Controller({ path: 'x', ... })`.
fn controller_path_arg(call: &CallExpression<'_>) -> PathArg {
    match first_argument(call).and_then(Argument::as_expression) {
        Some(Expression::ObjectExpression(options)) => options_path(options),
        _ => first_path_arg(call),
    }
}

fn options_path(options: &ObjectExpression<'_>) -> PathArg {
    let mut path = None;
    for property in &options.properties {
        match property {
            // A spread may contain or override `path`.
            ObjectPropertyKind::SpreadProperty(_) => {
                return PathArg::Dynamic("spread inside the options object");
            }
            ObjectPropertyKind::ObjectProperty(property) => {
                if property.computed {
                    return PathArg::Dynamic("computed key in the options object");
                }
                if property.key.static_name().as_deref() == Some("path") {
                    path = Some(path_from_expression(&property.value));
                }
            }
        }
    }
    path.unwrap_or_else(|| PathArg::Static(String::new()))
}

fn parameter_name(call: &CallExpression<'_>) -> ParameterName {
    let Some(argument) = first_argument(call) else {
        return ParameterName::Unnamed;
    };
    match argument.as_expression() {
        // `@Body(new ValidationPipe())`: NestJS accepts a pipe in the key
        // position, and a `new` expression can never be a string key.
        Some(Expression::NewExpression(_)) => ParameterName::Unnamed,
        Some(expression) => {
            static_string(expression).map_or(ParameterName::Unresolved, ParameterName::Named)
        }
        None => ParameterName::Unresolved,
    }
}

/// Byte offsets of line starts, for offset → (line, column).
struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    /// Lines end at `\n`, `\r\n` or a lone `\r`.
    fn new(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut starts = vec![0];
        for (index, byte) in bytes.iter().enumerate() {
            let is_break = match byte {
                b'\n' => true,
                b'\r' => bytes.get(index + 1) != Some(&b'\n'),
                _ => false,
            };
            if is_break && let Ok(next) = u32::try_from(index + 1) {
                starts.push(next);
            }
        }
        Self { starts }
    }

    /// 1-based line and column (in Unicode scalar values) of a byte offset.
    fn locate(&self, source: &str, offset: u32) -> (u32, u32) {
        let line = self.starts.partition_point(|&start| start <= offset);
        // `starts[0] == 0 <= offset`, so `line >= 1`.
        let line_start = self
            .starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        let column = source
            .get(line_start as usize..offset as usize)
            .map_or(offset.saturating_sub(line_start), |text| {
                u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
            });
        (
            u32::try_from(line).unwrap_or(u32::MAX),
            column.saturating_add(1),
        )
    }
}

#[cfg(test)]
mod tests;
