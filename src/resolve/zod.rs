//! Zod schema static analysis.
//!
//! Extracts object property names from `z.object({ ... })` calls and related
//! transformations (extend, pick, omit, partial, required, strict, passthrough, strip).
//! Also handles `z.infer<typeof Schema>` type aliases and `createZodDto(Schema)` classes.

use std::collections::{HashMap, HashSet};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, CallExpression, Declaration, Expression, ImportDeclarationSpecifier,
    ModuleExportName, ObjectPropertyKind, Statement, TSType, TSTypeName, VariableDeclarationKind,
};
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::resolve::limits::{MAX_DTO_PROPERTIES, MAX_SOURCE_BYTES};

/// Result of analyzing a Zod schema expression.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZodSchema {
    /// The property names of the object schema, in order.
    pub properties: Vec<String>,
    /// Whether this schema is the result of transformations that we couldn't fully resolve.
    /// If true, the properties may be incomplete/unknown.
    pub uncertain: bool,
    /// If this schema is a `createZodDto(ImportedSchema)`, this records the local name
    /// of the imported schema and the module it's imported from.
    pub import_ref: Option<ImportedSchemaRef>,
}

/// Reference to an imported schema that needs cross-file resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSchemaRef {
    /// The local name of the imported schema (e.g., "CreateUserSchema").
    pub local_name: String,
    /// The module specifier it's imported from (e.g., "./create-user.schema").
    pub from_specifier: String,
}

/// Analyze a source file for Zod schemas and related constructs.
/// Returns a map from exported name (const/type/class) to ZodSchema.
pub fn analyze_zod_in_file(source: &str) -> HashMap<String, ZodSchema> {
    if source.len() > MAX_SOURCE_BYTES {
        return HashMap::new();
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if parsed.panicked || !parsed.errors.is_empty() {
        return HashMap::new();
    }
    analyze_zod_program(&parsed.program)
}

/// Analyze an already-parsed program for Zod schemas and related constructs.
///
/// The single-parse entry point: module analysis parses a dependency once and
/// hands the same program to this function, [`extract_imports_exports`], and
/// local DTO extraction, instead of re-parsing per analyzer.
pub fn analyze_zod_program(program: &oxc_ast::ast::Program<'_>) -> HashMap<String, ZodSchema> {
    let mut analyzer = ZodAnalyzer::new();
    analyzer.analyze(program);
    analyzer.schemas
}

struct ZodAnalyzer<'ast> {
    schemas: HashMap<String, ZodSchema>,
    /// Local variable bindings: name -> ZodSchema (for const schemas)
    bindings: HashMap<String, ZodSchema>,
    /// Imported names from 'zod' or 'zod/*' -> local binding name
    zod_imports: HashMap<String, String>, // original export name -> local name
    /// Imported names from 'nestjs-zod' -> local binding name
    nestjs_zod_imports: HashMap<String, String>,
    /// All imports: local name -> (imported name, specifier)
    all_imports: HashMap<String, (String, String)>,
    /// Phantom data to tie the AST lifetime
    _phantom: std::marker::PhantomData<&'ast ()>,
}

impl<'ast> ZodAnalyzer<'ast> {
    fn new() -> Self {
        Self {
            schemas: HashMap::new(),
            bindings: HashMap::new(),
            zod_imports: HashMap::new(),
            nestjs_zod_imports: HashMap::new(),
            all_imports: HashMap::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    fn analyze(&mut self, program: &oxc_ast::ast::Program<'ast>) {
        // First pass: collect imports
        for stmt in &program.body {
            if let Statement::ImportDeclaration(decl) = stmt {
                self.collect_imports(decl);
            }
        }

        // Second pass: analyze declarations
        for stmt in &program.body {
            self.analyze_statement(stmt);
        }
    }

    fn collect_imports(&mut self, decl: &oxc_ast::ast::ImportDeclaration) {
        let specifier = decl.source.value.as_str();
        let is_zod = specifier == "zod" || specifier.starts_with("zod/");
        let is_nestjs_zod = specifier == "nestjs-zod" || specifier.starts_with("nestjs-zod/");

        if let Some(specs) = &decl.specifiers {
            for spec in specs {
                match spec {
                    ImportDeclarationSpecifier::ImportSpecifier(spec) => {
                        let imported = match &spec.imported {
                            ModuleExportName::IdentifierName(id) => id.name.as_str(),
                            ModuleExportName::IdentifierReference(id) => id.name.as_str(),
                            ModuleExportName::StringLiteral(lit) => lit.value.as_str(),
                        };
                        let local = spec.local.name.as_str();
                        if is_zod {
                            self.zod_imports
                                .insert(imported.to_owned(), local.to_owned());
                        } else if is_nestjs_zod {
                            self.nestjs_zod_imports
                                .insert(imported.to_owned(), local.to_owned());
                        }
                        // Track all imports for cross-file schema resolution
                        self.all_imports.insert(
                            local.to_owned(),
                            (imported.to_owned(), specifier.to_owned()),
                        );
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(spec) => {
                        let local = spec.local.name.as_str();
                        if is_zod {
                            self.zod_imports.insert("*".to_owned(), local.to_owned());
                        } else if is_nestjs_zod {
                            self.nestjs_zod_imports
                                .insert("*".to_owned(), local.to_owned());
                        }
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(spec) => {
                        let local = spec.local.name.as_str();
                        if is_zod {
                            self.zod_imports
                                .insert("default".to_owned(), local.to_owned());
                        } else if is_nestjs_zod {
                            self.nestjs_zod_imports
                                .insert("default".to_owned(), local.to_owned());
                        }
                    }
                }
            }
        }
    }

    fn analyze_statement(&mut self, stmt: &Statement<'ast>) {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                for var_decl in &decl.declarations {
                    if let BindingPattern::BindingIdentifier(id) = &var_decl.id {
                        let name = id.name.to_string();
                        if let Some(init) = &var_decl.init
                            && let Some(schema) = self.analyze_zod_call(init)
                        {
                            // Track binding for later reference
                            self.bindings.insert(name.clone(), schema.clone());
                            // If exported, add to schemas
                            if decl.kind == VariableDeclarationKind::Const {
                                self.schemas.insert(name, schema);
                            }
                        }
                    }
                }
            }
            Statement::TSTypeAliasDeclaration(alias) => {
                let name = alias.id.name.to_string();
                if let Some(schema) = self.analyze_type_alias(&alias.type_annotation) {
                    self.schemas.insert(name, schema);
                }
            }
            Statement::ClassDeclaration(cls) => {
                if let Some(id) = &cls.id {
                    let name = id.name.to_string();
                    if let Some(schema) = self.analyze_class_declaration(cls) {
                        self.schemas.insert(name, schema);
                    }
                }
            }
            Statement::ExportNamedDeclaration(export) => {
                // Handle `export const X = ...`, `export type X = ...`, `export class X ...` at top level
                if let Some(decl) = &export.declaration {
                    match decl {
                        Declaration::VariableDeclaration(var) => {
                            if var.kind == VariableDeclarationKind::Const {
                                for var_decl in &var.declarations {
                                    if let BindingPattern::BindingIdentifier(id) = &var_decl.id {
                                        let name = id.name.to_string();
                                        if let Some(init) = &var_decl.init
                                            && let Some(schema) = self.analyze_zod_call(init)
                                        {
                                            // Track binding for later reference (for chained transformations)
                                            self.bindings.insert(name.clone(), schema.clone());
                                            self.schemas.insert(name, schema);
                                        }
                                    }
                                }
                            }
                        }
                        Declaration::TSTypeAliasDeclaration(alias) => {
                            // Handle `export type X = ...`
                            let name = alias.id.name.to_string();
                            if let Some(schema) = self.analyze_type_alias(&alias.type_annotation) {
                                self.schemas.insert(name, schema);
                            }
                        }
                        Declaration::ClassDeclaration(cls) => {
                            // Handle `export class X extends createZodDto(...)`
                            if let Some(id) = &cls.id {
                                let name = id.name.to_string();
                                if let Some(schema) = self.analyze_class_declaration(cls) {
                                    self.schemas.insert(name, schema);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn analyze_type_alias(&self, type_ann: &TSType<'ast>) -> Option<ZodSchema> {
        let ts_type = type_ann;
        // Look for `z.infer<typeof SchemaName>` or `infer<typeof SchemaName>`
        if let TSType::TSTypeReference(type_ref) = ts_type {
            let type_name = match &type_ref.type_name {
                TSTypeName::IdentifierReference(ident) => ident.name.as_str(),
                TSTypeName::QualifiedName(qn) => qn.right.name.as_str(),
                _ => return None,
            };
            if type_name != "infer" {
                return None;
            }
            // Check type arguments: <typeof SchemaName>
            if let Some(type_args) = &type_ref.type_arguments
                // type_args is Box<TSTypeParameterInstantiation> with params field
                && let Some(TSType::TSTypeQuery(type_query)) = type_args.params.first()
            {
                // expr_name is TSTypeQueryExprName which inherits TSTypeName
                // We need to match using the TSTypeName variants
                if let oxc_ast::ast::TSTypeQueryExprName::IdentifierReference(ref_ident) =
                    &type_query.expr_name
                {
                    let schema_name = ref_ident.name.to_string();
                    // Resolve the schema name in bindings
                    return self.bindings.get(&schema_name).cloned();
                }
            }
        }
        None
    }

    fn analyze_class_declaration(&self, cls: &oxc_ast::ast::Class<'ast>) -> Option<ZodSchema> {
        // Check for `extends createZodDto(SchemaExpr)`
        if let Some(super_class) = &cls.super_class
            && let Expression::CallExpression(call) = super_class
            && let Expression::Identifier(callee) = &call.callee
        {
            // Check if callee is imported from 'nestjs-zod' as createZodDto
            let callee_name = callee.name.as_str();
            if self
                .nestjs_zod_imports
                .get("createZodDto")
                .is_some_and(|local_name| local_name == callee_name)
            {
                // First argument is the schema expression
                if let Some(first_arg) = call.arguments.first()
                    && let Some(expr) = first_arg.as_expression()
                {
                    // Try to analyze as a Zod call (e.g., z.object(...))
                    if let Some(schema) = self.analyze_zod_call(expr) {
                        return Some(schema);
                    }
                    // If it's an identifier, look it up in bindings (e.g., CreateUserSchema)
                    if let Expression::Identifier(ident) = expr {
                        let schema_name = ident.name.to_string();
                        if let Some(schema) = self.bindings.get(&schema_name).cloned() {
                            return Some(schema);
                        }
                        // If not in local bindings, check if it's an imported schema
                        if let Some((imported_name, from_specifier)) =
                            self.all_imports.get(&schema_name)
                        {
                            return Some(ZodSchema {
                                properties: Vec::new(),
                                uncertain: false,
                                import_ref: Some(ImportedSchemaRef {
                                    local_name: imported_name.clone(),
                                    from_specifier: from_specifier.clone(),
                                }),
                            });
                        }
                    }
                }
            }
        }
        None
    }

    /// Analyze a potential Zod call expression.
    fn analyze_zod_call(&self, expr: &Expression<'ast>) -> Option<ZodSchema> {
        match expr {
            Expression::CallExpression(call) => self.analyze_call(call),
            // Also handle identifiers that might reference a schema (for createZodDto case)
            Expression::Identifier(_) => None, // Handled in analyze_class_declaration
            _ => None,
        }
    }

    /// Analyze a CallExpression that might be a Zod schema or a transformation chain.
    fn analyze_call(&self, call: &CallExpression<'ast>) -> Option<ZodSchema> {
        // First check if it's a direct z.object(...) call
        if let Some(schema) = self.analyze_direct_zod_object(call) {
            return Some(schema);
        }

        // Check if it's a chained transformation on a known schema binding
        if let Expression::StaticMemberExpression(mem) = &call.callee
            && let Expression::Identifier(obj_ident) = &mem.object
        {
            let base_name = obj_ident.name.to_string();
            if let Some(base_schema) = self.bindings.get(&base_name) {
                // Collect the full chain of member calls
                let chain = self.collect_chain(call);
                return Some(self.analyze_chained(base_schema.clone(), &chain));
            }
        }

        None
    }

    /// Analyze a direct z.object(...) call (or aliased variants).
    fn analyze_direct_zod_object(&self, call: &CallExpression<'ast>) -> Option<ZodSchema> {
        let (base_name, method) = self.extract_callee_name(&call.callee)?;
        let is_zod_base = self.zod_imports.values().any(|v| v == &base_name);
        let is_object_import = self
            .zod_imports
            .get("object")
            .is_some_and(|v| v == &base_name);

        let is_zod_call =
            (is_zod_base && method == "object") || (is_object_import && method.is_empty());

        if !is_zod_call {
            return None;
        }

        // First argument should be an object literal
        let first_arg = call.arguments.first()?.as_expression()?;
        let obj = match first_arg {
            Expression::ObjectExpression(obj) => obj,
            _ => return None,
        };

        let mut props = Vec::new();
        for prop in &obj.properties {
            if let ObjectPropertyKind::ObjectProperty(p) = prop
                && let Some(name) = self.extract_property_name(&p.key)
            {
                props.push(name);
                if props.len() >= MAX_DTO_PROPERTIES {
                    break;
                }
            }
        }
        Some(ZodSchema {
            properties: props,
            uncertain: false,
            import_ref: None,
        })
    }

    /// Collect a chain of chained method calls from a CallExpression.
    /// E.g., for `BaseSchema.extend({...}).pick({...})`, returns both CallExpressions.
    fn collect_chain<'a>(
        &'a self,
        call: &'a CallExpression<'ast>,
    ) -> Vec<&'a CallExpression<'ast>> {
        let mut chain = Vec::new();
        let mut current: &'a CallExpression<'ast> = call;

        loop {
            chain.push(current);
            // Check if the callee is a member expression on another call
            if let Expression::StaticMemberExpression(mem) = &current.callee
                && let Expression::CallExpression(inner_call) = &mem.object
            {
                current = inner_call;
                continue;
            }
            break;
        }

        // Reverse so we process from base outward
        chain.reverse();
        chain
    }

    fn extract_callee_name(&self, callee: &Expression<'ast>) -> Option<(String, String)> {
        match callee {
            Expression::Identifier(id) => Some((id.name.to_string(), String::new())),
            Expression::StaticMemberExpression(mem) => {
                let obj_name = match &mem.object {
                    Expression::Identifier(id) => id.name.to_string(),
                    _ => return None,
                };
                let prop_name = mem.property.name.to_string();
                Some((obj_name, prop_name))
            }
            Expression::ComputedMemberExpression(_) => None, // computed keys not supported
            Expression::PrivateFieldExpression(_) => None,   // private fields not supported
            _ => None,
        }
    }

    fn extract_property_name(&self, key: &oxc_ast::ast::PropertyKey<'ast>) -> Option<String> {
        match key {
            oxc_ast::ast::PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
            oxc_ast::ast::PropertyKey::StringLiteral(lit) => Some(lit.value.to_string()),
            oxc_ast::ast::PropertyKey::NumericLiteral(lit) => Some(lit.value.to_string()),
            _ => None, // computed keys skipped
        }
    }

    /// Analyze chained transformations: .extend(), .pick(), .omit(), .partial(), etc.
    fn analyze_chained(&self, base: ZodSchema, chain: &[&CallExpression<'ast>]) -> ZodSchema {
        let mut current = base;
        for call in chain {
            let (_base_name, method) = match self.extract_callee_name(&call.callee) {
                Some(n) => n,
                None => {
                    return ZodSchema {
                        properties: vec![],
                        uncertain: true,
                        import_ref: None,
                    };
                }
            };
            // The base_name should be the previous call's result (we track via chaining)
            // For simplicity, we just check method name.
            match method.as_str() {
                "extend" => {
                    if let Some(arg) = call.arguments.first().and_then(|a| a.as_expression()) {
                        if let Expression::ObjectExpression(obj) = arg {
                            for prop in &obj.properties {
                                if let ObjectPropertyKind::ObjectProperty(p) = prop
                                    && let Some(name) = self.extract_property_name(&p.key)
                                    && !current.properties.contains(&name)
                                {
                                    current.properties.push(name);
                                    if current.properties.len() >= MAX_DTO_PROPERTIES {
                                        break;
                                    }
                                }
                            }
                        } else {
                            current.uncertain = true;
                        }
                    }
                }
                "pick" => {
                    // pick({ a: true, b: true }) -> keep only those keys
                    if let Some(arg) = call.arguments.first().and_then(|a| a.as_expression()) {
                        if let Expression::ObjectExpression(obj) = arg {
                            let mut keep = HashSet::new();
                            for prop in &obj.properties {
                                if let ObjectPropertyKind::ObjectProperty(p) = prop
                                    && let Some(name) = self.extract_property_name(&p.key)
                                {
                                    // Check if value is `true` literal
                                    if let Expression::BooleanLiteral(b) = &p.value {
                                        if b.value {
                                            keep.insert(name);
                                        }
                                    } else {
                                        // Non-literal value: uncertain
                                        current.uncertain = true;
                                    }
                                }
                            }
                            if !current.uncertain {
                                current.properties.retain(|p| keep.contains(p));
                            }
                        } else {
                            current.uncertain = true;
                        }
                    }
                }
                "omit" => {
                    if let Some(arg) = call.arguments.first().and_then(|a| a.as_expression()) {
                        if let Expression::ObjectExpression(obj) = arg {
                            let mut remove = HashSet::new();
                            for prop in &obj.properties {
                                if let ObjectPropertyKind::ObjectProperty(p) = prop
                                    && let Some(name) = self.extract_property_name(&p.key)
                                {
                                    // Check if value is `true` literal
                                    if let Expression::BooleanLiteral(b) = &p.value {
                                        if b.value {
                                            remove.insert(name);
                                        }
                                    } else {
                                        current.uncertain = true;
                                    }
                                }
                            }
                            if !current.uncertain {
                                current.properties.retain(|p| !remove.contains(p));
                            }
                        } else {
                            current.uncertain = true;
                        }
                    }
                }
                "partial" | "required" | "strict" | "passthrough" | "strip" => {
                    // These don't change property names
                }
                _ => {
                    // Unknown method: uncertain
                    current.uncertain = true;
                }
            }
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_z_object() {
        let source = r#"
import { z } from 'zod';
export const CreateUserSchema = z.object({
  name: z.string(),
  email: z.string().email(),
  age: z.number(),
});
"#;
        let schemas = analyze_zod_in_file(source);
        assert!(schemas.contains_key("CreateUserSchema"));
        let schema = schemas.get("CreateUserSchema").unwrap();
        assert_eq!(schema.properties, vec!["name", "email", "age"]);
        assert!(!schema.uncertain);
    }

    #[test]
    fn extracts_aliased_z_import() {
        let source = r#"
import { z as zod } from 'zod';
export const CreateUserSchema = zod.object({
  name: zod.string(),
});
"#;
        let schemas = analyze_zod_in_file(source);
        let schema = schemas.get("CreateUserSchema").unwrap();
        assert_eq!(schema.properties, vec!["name"]);
    }

    #[test]
    fn extracts_named_object_import() {
        let source = r#"
import { object, string } from 'zod';
export const CreateUserSchema = object({
  name: string(),
});
"#;
        let schemas = analyze_zod_in_file(source);
        let schema = schemas.get("CreateUserSchema").unwrap();
        assert_eq!(schema.properties, vec!["name"]);
    }

    #[test]
    fn extracts_z_infer_type_alias() {
        let source = r#"
import { z } from 'zod';
const CreateUserSchema = z.object({ name: z.string() });
export type CreateUserDto = z.infer<typeof CreateUserSchema>;
"#;
        let schemas = analyze_zod_in_file(source);
        assert!(schemas.contains_key("CreateUserDto"));
        let schema = schemas.get("CreateUserDto").unwrap();
        assert_eq!(schema.properties, vec!["name"]);
    }

    #[test]
    fn extracts_create_zod_dto() {
        let source = r#"
import { createZodDto } from 'nestjs-zod';
import { z } from 'zod';
const CreateUserSchema = z.object({ name: z.string() });
export class CreateUserDto extends createZodDto(CreateUserSchema) {}
"#;
        let schemas = analyze_zod_in_file(source);
        assert!(schemas.contains_key("CreateUserDto"));
        let schema = schemas.get("CreateUserDto").unwrap();
        assert_eq!(schema.properties, vec!["name"]);
    }

    #[test]
    fn does_not_recognize_unimported_create_zod_dto() {
        let source = r#"
import { z } from 'zod';
const CreateUserSchema = z.object({ name: z.string() });
export class CreateUserDto extends createZodDto(CreateUserSchema) {}
"#;
        let schemas = analyze_zod_in_file(source);
        assert!(!schemas.contains_key("CreateUserDto"));
    }

    #[test]
    fn handles_extend_transformation() {
        let source = r#"
import { z } from 'zod';
export const BaseSchema = z.object({ name: z.string() });
export const ExtendedSchema = BaseSchema.extend({ email: z.string() });
"#;
        let schemas = analyze_zod_in_file(source);
        let base = schemas.get("BaseSchema").unwrap();
        assert_eq!(base.properties, vec!["name"]);
        let extended = schemas.get("ExtendedSchema").unwrap();
        assert_eq!(extended.properties, vec!["name", "email"]);
    }

    #[test]
    fn handles_pick_transformation() {
        let source = r#"
import { z } from 'zod';
export const BaseSchema = z.object({ name: z.string(), email: z.string(), age: z.number() });
export const PickedSchema = BaseSchema.pick({ name: true, email: true });
"#;
        let schemas = analyze_zod_in_file(source);
        let picked = schemas.get("PickedSchema").unwrap();
        assert_eq!(picked.properties, vec!["name", "email"]);
    }

    #[test]
    fn handles_omit_transformation() {
        let source = r#"
import { z } from 'zod';
export const BaseSchema = z.object({ name: z.string(), email: z.string(), password: z.string() });
export const OmittedSchema = BaseSchema.omit({ password: true });
"#;
        let schemas = analyze_zod_in_file(source);
        let omitted = schemas.get("OmittedSchema").unwrap();
        assert_eq!(omitted.properties, vec!["name", "email"]);
    }

    #[test]
    fn partial_required_strict_passthrough_strip_preserve_props() {
        let source = r#"
import { z } from 'zod';
export const BaseSchema = z.object({ name: z.string() });
export const PartialSchema = BaseSchema.partial();
export const RequiredSchema = BaseSchema.required();
export const StrictSchema = BaseSchema.strict();
export const PassthroughSchema = BaseSchema.passthrough();
export const StripSchema = BaseSchema.strip();
"#;
        let schemas = analyze_zod_in_file(source);
        for name in [
            "PartialSchema",
            "RequiredSchema",
            "StrictSchema",
            "PassthroughSchema",
            "StripSchema",
        ] {
            assert_eq!(schemas.get(name).unwrap().properties, vec!["name"]);
        }
    }

    #[test]
    fn unknown_method_marks_uncertain() {
        let source = r#"
import { z } from 'zod';
export const BaseSchema = z.object({ name: z.string() });
export const WeirdSchema = BaseSchema.unknownMethod();
"#;
        let schemas = analyze_zod_in_file(source);
        assert!(schemas.get("WeirdSchema").unwrap().uncertain);
    }

    #[test]
    fn non_literal_pick_value_marks_uncertain() {
        let source = r#"
import { z } from 'zod';
export const BaseSchema = z.object({ name: z.string(), email: z.string() });
const keys = { name: true };
export const PickedSchema = BaseSchema.pick(keys);
"#;
        let schemas = analyze_zod_in_file(source);
        assert!(schemas.get("PickedSchema").unwrap().uncertain);
    }
}
