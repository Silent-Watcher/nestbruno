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
    MethodDefinitionType, ObjectExpression, ObjectPropertyKind, Statement,
};
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};

use crate::diag::{Diagnostic, Severity};
use crate::route::{
    Controller, HttpMethod, ParameterKind, ParameterName, Route, RouteParameter, RoutePath,
    SourceLocation, compose_path,
};

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
pub fn extract_controllers(file: &Path, source: &str) -> FileAnalysis {
    if source.len() > MAX_SOURCE_BYTES {
        return FileAnalysis {
            controllers: Vec::new(),
            diagnostics: vec![too_large(file, source.len())],
        };
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();

    let mut extractor = Extractor {
        file: Arc::from(file),
        source,
        lines: OnceCell::new(),
        diagnostics: Vec::new(),
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
    extractor.finish(controllers)
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

struct Extractor<'s> {
    file: Arc<Path>,
    source: &'s str,
    /// Built on first use; most files never need it.
    lines: OnceCell<LineIndex>,
    diagnostics: Vec<Diagnostic>,
}

impl Extractor<'_> {
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

        let mut routes = Vec::new();
        for element in &class.body.body {
            if let ClassElement::MethodDefinition(method) = element
                && let Some(route) = self.route(&name, controller_path.as_deref(), method)
            {
                routes.push(route);
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

    /// Analyze one method; `None` when it is not a route.
    fn route(
        &mut self,
        controller: &str,
        controller_path: Option<&str>,
        method: &MethodDefinition<'_>,
    ) -> Option<Route> {
        if method.kind != MethodDefinitionKind::Method
            || method.r#type != MethodDefinitionType::MethodDefinition
            || method.r#static
        {
            return None;
        }

        let mut route_decorators = method.decorators.iter().filter_map(|decorator| {
            let (name, call) = decorator_call(decorator)?;
            Some((HttpMethod::from_decorator(name)?, decorator, call))
        });
        let (http_method, decorator, call) = route_decorators.next()?;
        let ignored = route_decorators.count();

        let Some(method_name) = method.key.static_name() else {
            self.warn(
                decorator.span,
                format!("route in {controller} has a computed method name; it is skipped"),
            );
            return None;
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
        for (index, parameter) in method.value.params.items.iter().enumerate() {
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
                parameters.push(RouteParameter { kind, name, index });
            }
        }

        Some(Route {
            method_name,
            http_method,
            path,
            parameters,
            source: self.location(decorator.span),
        })
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
