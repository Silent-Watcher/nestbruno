//! Import/export declaration extraction from Oxc AST.
//!
//! This module extracts all import/export information needed for cross-file
//! DTO resolution, without any semantic analysis.

use oxc_ast::ast::{
    BindingPattern, Declaration, ExportAllDeclaration, ExportNamedDeclaration, ImportDeclaration,
    ImportDeclarationSpecifier, ModuleExportName, Statement, VariableDeclarationKind,
};

/// An import declaration from a source file.
#[derive(Debug, Clone)]
pub struct ImportDecl {
    /// The module specifier (e.g., "./dto/create-user", "@users/dto", "zod").
    pub specifier: String,
    /// The local bindings created by this import.
    pub bindings: Vec<ImportBinding>,
    /// Whether this is a type-only import (`import type ...`).
    pub is_type_only: bool,
}

/// One imported name binding.
#[derive(Debug, Clone)]
pub struct ImportBinding {
    /// The local name in this file (e.g., `CreateUserDto` in `import { CreateUserDto }`).
    pub local: String,
    /// The original exported name from the module (e.g., `CreateUserDto` in
    /// `import { CreateUserDto as C }` -> local=`C`, imported=`CreateUserDto`).
    pub imported: String,
    /// Whether this is a namespace import (`import * as X`).
    pub is_namespace: bool,
}

/// An export declaration from a source file.
#[derive(Debug, Clone)]
pub enum ExportDecl {
    /// `export class X {}` / `export interface X {}` / `export const X = ...`
    Local { name: String, kind: ExportKind },
    /// `export { X, Y as Z } from './mod'`
    ReExport {
        from: String,
        specifiers: Vec<ExportSpec>,
    },
    /// `export * from './mod'`
    StarReExport(String),
}

/// What kind of local declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Class,
    Interface,
    TypeAlias,
    Const,
    Other,
}

/// One specifier in a re-export: `export { orig as local }`.
#[derive(Debug, Clone)]
pub struct ExportSpec {
    pub exported: String, // original name in the source module
    pub local: String,    // name as exported from this module
}

/// All import/export information from one module.
#[derive(Debug, Clone, Default)]
pub struct ModuleImportsExports {
    pub imports: Vec<ImportDecl>,
    pub exports: Vec<ExportDecl>,
}

/// Extract imports and exports from a parsed Oxc program.
pub fn extract_imports_exports(program: &oxc_ast::ast::Program<'_>) -> ModuleImportsExports {
    let mut result = ModuleImportsExports::default();
    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => {
                if let Some(import) = extract_import(decl) {
                    result.imports.push(import);
                }
            }
            Statement::ExportNamedDeclaration(decl) => {
                if let Some(exports) = extract_named_export(decl) {
                    result.exports.extend(exports);
                }
            }
            Statement::ExportAllDeclaration(decl) => {
                if let Some(spec) = extract_star_export(decl) {
                    result.exports.push(ExportDecl::StarReExport(spec));
                }
            }
            _ => {}
        }
    }
    result
}

fn extract_import(decl: &ImportDeclaration) -> Option<ImportDecl> {
    let specifier = decl.source.value.to_string();
    let mut bindings = Vec::new();
    let is_type_only = decl.import_kind.is_type();

    if let Some(specs) = &decl.specifiers {
        for spec in specs {
            match spec {
                ImportDeclarationSpecifier::ImportDefaultSpecifier(spec) => {
                    bindings.push(ImportBinding {
                        local: spec.local.name.to_string(),
                        imported: "default".to_owned(),
                        is_namespace: false,
                    });
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(spec) => {
                    bindings.push(ImportBinding {
                        local: spec.local.name.to_string(),
                        imported: "*".to_owned(),
                        is_namespace: true,
                    });
                }
                ImportDeclarationSpecifier::ImportSpecifier(spec) => {
                    let imported = match &spec.imported {
                        ModuleExportName::IdentifierName(id) => id.name.to_string(),
                        ModuleExportName::IdentifierReference(id) => id.name.to_string(),
                        ModuleExportName::StringLiteral(lit) => lit.value.to_string(),
                    };
                    bindings.push(ImportBinding {
                        local: spec.local.name.to_string(),
                        imported,
                        is_namespace: false,
                    });
                }
            }
        }
    }

    if bindings.is_empty() {
        // Side-effect import: `import './foo'` - no bindings to track
        return None;
    }
    Some(ImportDecl {
        specifier,
        bindings,
        is_type_only,
    })
}

fn extract_named_export(decl: &ExportNamedDeclaration) -> Option<Vec<ExportDecl>> {
    let mut out = Vec::new();
    // `export { X, Y as Z } from './mod'`
    if let Some(source) = &decl.source {
        let from = source.value.to_string();
        let mut specs = Vec::new();
        for spec in &decl.specifiers {
            // Oxc's ExportSpecifier has:
            // - `exported`: the name as exported FROM this module (public name)
            // - `local`: the name as imported FROM the source module (original name)
            // Our ExportSpec uses opposite convention:
            // - `exported`: original name in the source module
            // - `local`: name as exported from this module
            let oxc_exported = match &spec.exported {
                ModuleExportName::IdentifierName(id) => id.name.to_string(),
                ModuleExportName::IdentifierReference(id) => id.name.to_string(),
                ModuleExportName::StringLiteral(lit) => lit.value.to_string(),
            };
            let oxc_local = match &spec.local {
                ModuleExportName::IdentifierName(id) => id.name.to_string(),
                ModuleExportName::IdentifierReference(id) => id.name.to_string(),
                ModuleExportName::StringLiteral(lit) => lit.value.to_string(),
            };
            specs.push(ExportSpec {
                exported: oxc_local, // original name from source module
                local: oxc_exported, // name as exported from this module
            });
        }
        if !specs.is_empty() {
            out.push(ExportDecl::ReExport {
                from,
                specifiers: specs,
            });
        }
        return Some(out);
    }
    // `export class X`, `export interface X`, `export const X`
    if let Some(declaration) = &decl.declaration {
        match declaration {
            Declaration::ClassDeclaration(cls) => {
                if let Some(id) = &cls.id {
                    out.push(ExportDecl::Local {
                        name: id.name.to_string(),
                        kind: ExportKind::Class,
                    });
                }
            }
            Declaration::TSInterfaceDeclaration(iface) => {
                out.push(ExportDecl::Local {
                    name: iface.id.name.to_string(),
                    kind: ExportKind::Interface,
                });
            }
            Declaration::TSTypeAliasDeclaration(alias) => {
                out.push(ExportDecl::Local {
                    name: alias.id.name.to_string(),
                    kind: ExportKind::TypeAlias,
                });
            }
            Declaration::VariableDeclaration(var)
                // Only handle `export const X = ...` at top level
                if var.kind == VariableDeclarationKind::Const => {
                    for decl in &var.declarations {
                        if let BindingPattern::BindingIdentifier(id) = &decl.id {
                            out.push(ExportDecl::Local {
                                name: id.name.to_string(),
                                kind: ExportKind::Const,
                            });
                        }
                    }
                }
            _ => {}
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn extract_star_export(decl: &ExportAllDeclaration) -> Option<String> {
    Some(decl.source.value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    fn with_parsed<R>(text: &str, f: impl FnOnce(&oxc_ast::ast::Program<'_>) -> R) -> R {
        let allocator = Allocator::default();
        let program = Parser::new(&allocator, text, SourceType::ts())
            .parse()
            .program;
        f(&program)
    }

    #[test]
    fn extracts_named_imports() {
        with_parsed(
            r#"import { CreateUserDto } from './dto/create-user.dto';"#,
            |program| {
                let ie = extract_imports_exports(program);
                assert_eq!(ie.imports.len(), 1);
                let imp = &ie.imports[0];
                assert_eq!(imp.specifier, "./dto/create-user.dto");
                assert_eq!(imp.bindings.len(), 1);
                assert_eq!(imp.bindings[0].local, "CreateUserDto");
                assert_eq!(imp.bindings[0].imported, "CreateUserDto");
                assert!(!imp.is_type_only);
            },
        );
    }

    #[test]
    fn extracts_aliased_imports() {
        with_parsed(
            r#"import { CreateUserDto as CUD } from './dto/create-user.dto';"#,
            |program| {
                let ie = extract_imports_exports(program);
                assert_eq!(ie.imports[0].bindings[0].local, "CUD");
                assert_eq!(ie.imports[0].bindings[0].imported, "CreateUserDto");
            },
        );
    }

    #[test]
    fn extracts_type_only_imports() {
        with_parsed(
            r#"import type { CreateUserDto } from './dto/create-user.dto';"#,
            |program| {
                let ie = extract_imports_exports(program);
                assert!(ie.imports[0].is_type_only);
            },
        );
    }

    #[test]
    fn extracts_namespace_imports() {
        with_parsed(r#"import * as zod from 'zod';"#, |program| {
            let ie = extract_imports_exports(program);
            assert_eq!(ie.imports[0].bindings[0].local, "zod");
            assert_eq!(ie.imports[0].bindings[0].imported, "*");
            assert!(ie.imports[0].bindings[0].is_namespace);
        });
    }

    #[test]
    fn extracts_default_imports() {
        with_parsed(
            r#"import CreateUserDto from './dto/create-user.dto';"#,
            |program| {
                let ie = extract_imports_exports(program);
                assert_eq!(ie.imports[0].bindings[0].local, "CreateUserDto");
                assert_eq!(ie.imports[0].bindings[0].imported, "default");
            },
        );
    }

    #[test]
    fn extracts_reexports() {
        with_parsed(
            r#"export { CreateUserDto, UpdateUserDto as Update } from './dto';"#,
            |program| {
                let ie = extract_imports_exports(program);
                assert_eq!(ie.exports.len(), 1);
                if let ExportDecl::ReExport { from, specifiers } = &ie.exports[0] {
                    assert_eq!(from, "./dto");
                    assert_eq!(specifiers.len(), 2);
                    assert_eq!(specifiers[0].exported, "CreateUserDto");
                    assert_eq!(specifiers[0].local, "CreateUserDto");
                    assert_eq!(specifiers[1].exported, "UpdateUserDto");
                    assert_eq!(specifiers[1].local, "Update");
                } else {
                    panic!("expected ReExport");
                }
            },
        );
    }

    #[test]
    fn extracts_star_reexport() {
        with_parsed(r#"export * from './dto';"#, |program| {
            let ie = extract_imports_exports(program);
            assert_eq!(ie.exports.len(), 1);
            if let ExportDecl::StarReExport(from) = &ie.exports[0] {
                assert_eq!(from, "./dto");
            } else {
                panic!("expected StarReExport");
            }
        });
    }

    #[test]
    fn exports_local_class_interface_const() {
        with_parsed(
            r#"
export class CreateUserDto { name: string; }
export interface User { id: string; }
export const schema = z.object({ name: z.string() });
"#,
            |program| {
                let ie = extract_imports_exports(program);
                assert_eq!(ie.exports.len(), 3);
                assert!(matches!(
                    ie.exports[0],
                    ExportDecl::Local {
                        kind: ExportKind::Class,
                        ..
                    }
                ));
                assert!(matches!(
                    ie.exports[1],
                    ExportDecl::Local {
                        kind: ExportKind::Interface,
                        ..
                    }
                ));
                assert!(matches!(
                    ie.exports[2],
                    ExportDecl::Local {
                        kind: ExportKind::Const,
                        ..
                    }
                ));
            },
        );
    }

    #[test]
    fn check_reexport_ast() {
        with_parsed(
            r#"export { CreateUserDto, UpdateUserDto as Update } from './dto';"#,
            |program| {
                for stmt in &program.body {
                    if let oxc_ast::ast::Statement::ExportNamedDeclaration(decl) = stmt
                        && let Some(source) = &decl.source
                    {
                        println!("source: {}", source.value);
                        for spec in &decl.specifiers {
                            println!("  exported: {:?}", spec.exported);
                            println!("  local: {:?}", spec.local);
                        }
                    }
                }
            },
        );
    }

    #[test]
    fn check_import_ast() {
        with_parsed(
            r#"import { CreateUserDto as CUD, UpdateUserDto } from './dto';"#,
            |program| {
                for stmt in &program.body {
                    if let oxc_ast::ast::Statement::ImportDeclaration(decl) = stmt {
                        println!("source: {}", decl.source.value);
                        if let Some(specs) = &decl.specifiers {
                            for spec in specs {
                                if let oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(
                                    spec,
                                ) = spec
                                {
                                    println!("  local: {:?}", spec.local);
                                    println!("  imported: {:?}", spec.imported);
                                }
                            }
                        }
                    }
                }
            },
        );
    }
}
