//! Cross-file DTO resolution module.
//!
//! This module resolves DTO types (class, interface, type alias, Zod schema)
//! from imports in controller files, following import chains up to a bounded
//! depth. It integrates with the existing analysis pipeline to provide
//! `BodyShape::Fields` for cross-file DTOs.

pub mod imports;
pub mod limits;
pub mod module;
pub mod tsconfig;
pub mod zod;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::diag::{Diagnostic, Severity};
use crate::discovery::Matcher;
use crate::resolve::imports::{ModuleImportsExports, extract_imports_exports};
use crate::resolve::limits::*;
use crate::resolve::module::{ModuleResolveError, resolve_module};
use crate::resolve::tsconfig::TsConfig;
use crate::resolve::zod::analyze_zod_in_file;
use crate::route::RouteParameter;

/// Result of resolving a DTO type name to its properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DtoResolution {
    /// Successfully resolved to a class/interface with properties.
    Fields(Vec<String>),
    /// Resolved to a Zod schema with properties.
    ZodFields(Vec<String>),
    /// The type was found but has no extractable properties (e.g., type alias to primitive).
    Empty,
    /// Could not resolve the type (not found, circular, depth exceeded, etc.).
    Unknown,
    /// Resolution was uncertain due to unsupported transformations.
    Uncertain(Vec<String>),
}

/// Context for cross-file DTO resolution during analysis of one controller file.
pub struct DtoResolver<'a> {
    /// Project root directory.
    project_root: &'a Path,
    /// Source directory prefix (e.g., "src").
    source_prefix: &'a Path,
    /// Configuration.
    config: &'a Config,
    /// Discovery matcher for exclusion checks.
    matcher: &'a Matcher,
    /// Parsed tsconfig (if any).
    tsconfig: Option<TsConfig>,
    /// Cache of resolved modules: project-relative path -> ModuleAnalysis.
    module_cache: HashMap<PathBuf, ModuleAnalysis>,
    /// Resolution stack for cycle detection: (module_path, symbol_name) pairs
    /// currently being resolved. A cycle is detected when the same pair appears twice.
    resolution_stack: Vec<(PathBuf, String)>,
    /// Total bytes of resolved dependency sources.
    total_dependency_bytes: usize,
    /// Number of dependency files resolved.
    dependency_file_count: usize,
    /// Number of DTO declarations extracted.
    dto_declaration_count: usize,
    /// Diagnostics collected during resolution.
    diagnostics: Vec<Diagnostic>,
}

/// Analysis of one TypeScript module for DTO resolution.
#[derive(Debug, Clone)]
struct ModuleAnalysis {
    /// Imports and exports from the module.
    imports_exports: ModuleImportsExports,
    /// Zod schemas defined in this module.
    zod_schemas: HashMap<String, crate::resolve::zod::ZodSchema>,
    /// Class/interface/type alias property names (same-file DTO extraction).
    local_dtos: HashMap<String, Vec<String>>,
    /// Source text (for re-parsing if needed). Kept for future use.
    _source: String,
}

impl<'a> DtoResolver<'a> {
    /// Create a new resolver for a controller file analysis.
    pub fn new(
        project_root: &'a Path,
        source_prefix: &'a Path,
        config: &'a Config,
        matcher: &'a Matcher,
    ) -> Self {
        let tsconfig = TsConfig::load(project_root);
        Self {
            project_root,
            source_prefix,
            config,
            matcher,
            tsconfig,
            module_cache: HashMap::new(),
            resolution_stack: Vec::new(),
            total_dependency_bytes: 0,
            dependency_file_count: 0,
            dto_declaration_count: 0,
            diagnostics: Vec::new(),
        }
    }

    /// Resolve a type name (from a @Body parameter) to its properties.
    /// Returns DtoResolution with the properties or Unknown/Uncertain.
    pub fn resolve_type(&mut self, type_name: &str, from_file: &Path) -> DtoResolution {
        // First check same-file DTOs (already extracted in extract.rs)
        // The caller should pass same-file DTOs separately; this only does cross-file.

        // Find the import that brings `type_name` into scope
        let Some((imported_name, specifier)) = self.find_import_binding(type_name, from_file)
        else {
            return DtoResolution::Unknown;
        };

        // Resolve the module specifier
        let Some(module_path) = self.resolve_imported_module(&imported_name, &specifier, from_file)
        else {
            return DtoResolution::Unknown;
        };

        // Look up the exported symbol (analyzes module internally)
        self.lookup_export(&imported_name, &module_path, 0)
    }

    /// Get all diagnostics collected during resolution.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Find the import binding for a local type name in a file.
    /// Returns (imported_name, specifier) if found.
    fn find_import_binding(
        &mut self,
        type_name: &str,
        from_file: &Path,
    ) -> Option<(String, String)> {
        let analysis = self.analyze_module(from_file)?;
        for import in &analysis.imports_exports.imports {
            for binding in &import.bindings {
                if binding.local == type_name {
                    return Some((binding.imported.clone(), import.specifier.clone()));
                }
            }
        }
        None
    }

    /// Resolve an imported module specifier to a project-relative path.
    fn resolve_imported_module(
        &self,
        _imported_name: &str,
        specifier: &str,
        from_file: &Path,
    ) -> Option<PathBuf> {
        match resolve_module(
            specifier,
            from_file,
            self.source_prefix,
            self.config,
            self.matcher,
            self.tsconfig.as_ref(),
            self.project_root,
        ) {
            Ok(Some(path)) => Some(path),
            Ok(None) => None,
            Err(ModuleResolveError::EscapesProject) => {
                // Path escapes project - diagnostic but not fatal
                None
            }
            Err(ModuleResolveError::Io) => None,
        }
    }

    /// Analyze a module (parse, extract imports/exports, Zod schemas, local DTOs).
    fn analyze_module(&mut self, module_path: &Path) -> Option<&ModuleAnalysis> {
        // Check cache first
        if self.module_cache.contains_key(module_path) {
            return self.module_cache.get(module_path);
        }

        // Check limits
        if self.dependency_file_count >= MAX_DEPENDENCY_FILES {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                "dependency file limit reached; stopping cross-file resolution",
            );
            return None;
        }

        // Read source
        let abs_path = self.project_root.join(module_path);
        let source = match std::fs::read_to_string(&abs_path) {
            Ok(s) => s,
            Err(_) => {
                self.add_diagnostic(
                    Severity::Warning,
                    module_path,
                    "cannot read dependency file",
                );
                return None;
            }
        };

        if source.len() > MAX_SOURCE_BYTES {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                "dependency file too large; skipping",
            );
            return None;
        }

        // Check total bytes limit
        if self.total_dependency_bytes + source.len() > MAX_TOTAL_DEPENDENCY_BYTES {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                "total dependency bytes limit reached; stopping",
            );
            return None;
        }

        // Parse and analyze
        let allocator = oxc_allocator::Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&allocator, &source, oxc_span::SourceType::ts()).parse();

        let imports_exports = if parsed.panicked || !parsed.errors.is_empty() {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                "failed to parse dependency; skipping",
            );
            return None;
        } else {
            extract_imports_exports(&parsed.program)
        };

        let zod_schemas = analyze_zod_in_file(&source);

        // Extract same-file class/interface properties (like dto_properties in extract.rs)
        let local_dtos = self.extract_local_dtos(&parsed.program);

        let analysis = ModuleAnalysis {
            imports_exports,
            zod_schemas,
            local_dtos,
            _source: source.clone(),
        };

        self.total_dependency_bytes += source.len();
        self.dependency_file_count += 1;
        self.dto_declaration_count += analysis.local_dtos.len() + analysis.zod_schemas.len();

        if self.dto_declaration_count > MAX_DTO_DECLARATIONS {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                "DTO declaration limit reached",
            );
        }

        self.module_cache
            .insert(module_path.to_path_buf(), analysis);
        self.module_cache.get(module_path)
    }

    /// Extract class/interface properties from a module (same logic as dto_properties).
    /// Handles both plain and exported declarations.
    fn extract_local_dtos(
        &self,
        program: &oxc_ast::ast::Program<'_>,
    ) -> HashMap<String, Vec<String>> {
        let mut dtos = HashMap::new();
        for stmt in &program.body {
            // Check for exported class/interface: `export class X {}` / `export interface X {}`
            if let oxc_ast::ast::Statement::ExportNamedDeclaration(export) = stmt
                && let Some(declaration) = &export.declaration
            {
                match declaration {
                    oxc_ast::ast::Declaration::ClassDeclaration(cls) => {
                        if let Some(id) = &cls.id {
                            let mut props = Vec::new();
                            for elem in &cls.body.body {
                                if let oxc_ast::ast::ClassElement::PropertyDefinition(prop) = elem
                                    && let Some(name) = prop.key.static_name()
                                {
                                    props.push(name.to_string());
                                    if props.len() >= MAX_DTO_PROPERTIES {
                                        break;
                                    }
                                }
                            }
                            dtos.insert(id.name.to_string(), props);
                        }
                        continue;
                    }
                    oxc_ast::ast::Declaration::TSInterfaceDeclaration(iface) => {
                        let mut props = Vec::new();
                        for sig in &iface.body.body {
                            if let oxc_ast::ast::TSSignature::TSPropertySignature(prop) = sig
                                && let Some(name) = prop.key.static_name()
                            {
                                props.push(name.to_string());
                                if props.len() >= MAX_DTO_PROPERTIES {
                                    break;
                                }
                            }
                        }
                        dtos.insert(iface.id.name.to_string(), props);
                        continue;
                    }
                    _ => {}
                }
            }
            // Check for plain class/interface declarations (not exported)
            match stmt {
                oxc_ast::ast::Statement::ClassDeclaration(cls) => {
                    if let Some(id) = &cls.id {
                        let mut props = Vec::new();
                        for elem in &cls.body.body {
                            if let oxc_ast::ast::ClassElement::PropertyDefinition(prop) = elem
                                && let Some(name) = prop.key.static_name()
                            {
                                props.push(name.to_string());
                                if props.len() >= MAX_DTO_PROPERTIES {
                                    break;
                                }
                            }
                        }
                        dtos.insert(id.name.to_string(), props);
                    }
                }
                oxc_ast::ast::Statement::TSInterfaceDeclaration(iface) => {
                    let mut props = Vec::new();
                    for sig in &iface.body.body {
                        if let oxc_ast::ast::TSSignature::TSPropertySignature(prop) = sig
                            && let Some(name) = prop.key.static_name()
                        {
                            props.push(name.to_string());
                            if props.len() >= MAX_DTO_PROPERTIES {
                                break;
                            }
                        }
                    }
                    dtos.insert(iface.id.name.to_string(), props);
                }
                _ => {}
            }
        }
        dtos
    }

    /// Look up an exported symbol in a module, following re-exports.
    fn lookup_export(
        &mut self,
        name: &str,
        module_path: &Path,
        reexport_depth: usize,
    ) -> DtoResolution {
        if reexport_depth > MAX_REEXPORT_HOPS {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                format!("re-export depth exceeded for {name}"),
            );
            return DtoResolution::Unknown;
        }

        // Cycle detection: check if we're already resolving this (module, symbol) pair
        let key = (module_path.to_path_buf(), name.to_owned());
        if self.resolution_stack.iter().any(|k| k == &key) {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                format!("circular import detected for {name}; skipping"),
            );
            return DtoResolution::Unknown;
        }

        // Push to resolution stack
        self.resolution_stack.push(key);

        // Analyze the module
        let analysis_opt = self.analyze_module(module_path);
        let Some(analysis) = analysis_opt else {
            self.resolution_stack.pop();
            return DtoResolution::Unknown;
        };

        // Clone the data we need to avoid borrow conflicts
        let local_dtos = analysis.local_dtos.clone();
        let zod_schemas = analysis.zod_schemas.clone();
        let exports = analysis.imports_exports.exports.clone();

        // 1. Check Zod schemas first (createZodDto classes are recorded here)
        if let Some(zod_schema) = zod_schemas.get(name) {
            // If this schema has an import_ref, resolve the referenced schema
            if let Some(import_ref) = &zod_schema.import_ref {
                self.resolution_stack.pop();
                // Resolve the imported schema from its module
                let target_module = self.resolve_imported_module(
                    &import_ref.local_name,
                    &import_ref.from_specifier,
                    module_path,
                );
                if let Some(target_path) = target_module {
                    return self.lookup_export(
                        &import_ref.local_name,
                        &target_path,
                        reexport_depth + 1,
                    );
                }
                return DtoResolution::Unknown;
            }
            self.resolution_stack.pop();
            if zod_schema.uncertain {
                return DtoResolution::Uncertain(zod_schema.properties.clone());
            }
            if zod_schema.properties.is_empty() {
                return DtoResolution::Empty;
            }
            return DtoResolution::ZodFields(zod_schema.properties.clone());
        }

        // 2. Check local DTOs (class/interface)
        if let Some(props) = local_dtos.get(name) {
            self.resolution_stack.pop();
            if !props.is_empty() {
                return DtoResolution::Fields(props.clone());
            }
            // Empty props - could be a createZodDto class, fall through to re-exports/Unknown
        }

        // 3. Check re-exports - collect the ones we need to follow
        let mut reexports_to_follow = Vec::new();
        for export in &exports {
            match export {
                crate::resolve::imports::ExportDecl::ReExport { from, specifiers } => {
                    for spec in specifiers {
                        if spec.local == name || spec.exported == name {
                            reexports_to_follow.push((spec.exported.clone(), from.clone()));
                        }
                    }
                }
                crate::resolve::imports::ExportDecl::StarReExport(from) => {
                    reexports_to_follow.push((name.to_owned(), from.clone()));
                }
                crate::resolve::imports::ExportDecl::Local { .. } => {
                    // Already checked in local_dtos and zod_schemas
                }
            }
        }

        // Follow re-exports (only the first match is followed; multiple re-exports of the
        // same name is ambiguous and treated as unknown)
        if let Some((exported_name, from)) = reexports_to_follow.into_iter().next() {
            let target_module = self.resolve_imported_module(&exported_name, &from, module_path);
            if let Some(target_path) = target_module {
                let result = self.lookup_export(&exported_name, &target_path, reexport_depth + 1);
                self.resolution_stack.pop();
                return result;
            }
        }
        self.resolution_stack.pop();
        DtoResolution::Unknown
    }

    fn add_diagnostic(&mut self, severity: Severity, path: &Path, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::new(
            severity,
            Some(path.to_path_buf()),
            message.into(),
        ));
    }
}

/// Result of resolving cross-file DTOs for a route's body parameters.
#[derive(Debug, Default)]
pub struct CrossFileDtoResolutions {
    /// Mapping from parameter index to its resolved DTO properties.
    pub resolutions: HashMap<usize, DtoResolution>,
    /// Diagnostics collected during resolution.
    pub diagnostics: Vec<Diagnostic>,
}

/// Integrate cross-file DTO resolution into the extractor.
/// This is called from `nest/extract.rs` after same-file DTO extraction.
pub fn resolve_cross_file_dtos(
    project_root: &Path,
    source_prefix: &Path,
    config: &Config,
    matcher: &Matcher,
    controller_file: &Path,
    body_params: &[RouteParameter],
    same_file_dtos: &HashMap<String, Vec<String>>,
) -> CrossFileDtoResolutions {
    let mut resolver = DtoResolver::new(project_root, source_prefix, config, matcher);
    let mut resolutions = HashMap::new();

    for param in body_params {
        if param.kind != crate::route::ParameterKind::Body {
            continue;
        }
        let Some(type_name) = &param.type_name else {
            continue;
        };

        // Check same-file DTOs first (already done in extract.rs, but double-check)
        if let Some(props) = same_file_dtos.get(type_name)
            && !props.is_empty()
        {
            // Same-file wins; already handled
            continue;
        }

        // Resolve cross-file
        let resolution = resolver.resolve_type(type_name, controller_file);
        match resolution {
            DtoResolution::Fields(props) | DtoResolution::ZodFields(props) => {
                resolutions.insert(param.index, DtoResolution::Fields(props));
            }
            DtoResolution::Uncertain(props) => {
                resolutions.insert(param.index, DtoResolution::Uncertain(props));
                resolver.diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    Some(controller_file.to_path_buf()),
                    format!("Zod schema resolution for {type_name} is uncertain due to unsupported transformations; using available properties"),
                ));
            }
            _ => {}
        }
    }

    CrossFileDtoResolutions {
        resolutions,
        diagnostics: resolver.diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn setup_project() -> (tempfile::TempDir, PathBuf, Config, Matcher) {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();

        let config = Config {
            source: PathBuf::from("src"),
            bruno: PathBuf::from("bruno"),
            base_url: "{{baseUrl}}".into(),
            include: vec!["**/*.controller.ts".into()],
            exclude: vec!["**/dist/**".into(), "**/node_modules/**".into()],
        };
        let matcher = Matcher::new(&config).unwrap();
        (dir, root, config, matcher)
    }

    #[test]
    fn resolves_class_dto_from_relative_import() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/create-user.dto.ts"),
            "export class CreateUserDto { name: string; email: string; }",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { CreateUserDto } from './dto/create-user.dto';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: CreateUserDto) {}
}
"#,
        )
        .unwrap();

        let mut resolver = DtoResolver::new(&root, Path::new("src"), &config, &matcher);
        let resolution =
            resolver.resolve_type("CreateUserDto", Path::new("src/users/users.controller.ts"));
        assert_eq!(
            resolution,
            DtoResolution::Fields(vec!["name".into(), "email".into()])
        );
    }

    #[test]
    fn resolves_interface_dto_from_relative_import() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/user.interface.ts"),
            "export interface UserDto { name: string; }",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { UserDto } from './dto/user.interface';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: UserDto) {}
}
"#,
        )
        .unwrap();

        let mut resolver = DtoResolver::new(&root, Path::new("src"), &config, &matcher);
        let resolution =
            resolver.resolve_type("UserDto", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Fields(vec!["name".into()]));
    }

    #[test]
    fn resolves_zod_schema_from_relative_import() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/create-user.schema.ts"),
            r#"
import { z } from 'zod';
export const CreateUserSchema = z.object({ name: z.string(), email: z.string() });
export type CreateUserDto = z.infer<typeof CreateUserSchema>;
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import type { CreateUserDto } from './dto/create-user.schema';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: CreateUserDto) {}
}
"#,
        )
        .unwrap();

        let mut resolver = DtoResolver::new(&root, Path::new("src"), &config, &matcher);
        let resolution =
            resolver.resolve_type("CreateUserDto", Path::new("src/users/users.controller.ts"));
        assert_eq!(
            resolution,
            DtoResolution::ZodFields(vec!["name".into(), "email".into()])
        );
    }

    #[test]
    fn follows_reexport_chain() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/create-user.dto.ts"),
            "export class CreateUserDto { name: string; email: string; }",
        )
        .unwrap();
        fs::write(
            root.join("src/users/dto/index.ts"),
            "export { CreateUserDto } from './create-user.dto';",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { CreateUserDto } from './dto';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: CreateUserDto) {}
}
"#,
        )
        .unwrap();

        let mut resolver = DtoResolver::new(&root, Path::new("src"), &config, &matcher);
        let resolution =
            resolver.resolve_type("CreateUserDto", Path::new("src/users/users.controller.ts"));
        assert_eq!(
            resolution,
            DtoResolution::Fields(vec!["name".into(), "email".into()])
        );
    }

    #[test]
    fn respects_reexport_depth_limit() {
        let (_dir, root, config, matcher) = setup_project();
        // Create a chain of 5 re-exports (exceeds MAX_REEXPORT_HOPS=4)
        for i in 0..6 {
            let dir = root.join("src").join(format!("level{i}"));
            fs::create_dir_all(&dir).unwrap();
            if i == 0 {
                fs::write(dir.join("dto.ts"), "export class Dto { a: string; }").unwrap();
            } else {
                let from = format!("../level{}/dto", i - 1);
                fs::write(
                    dir.join("dto.ts"),
                    format!("export {{ Dto }} from '{}';", from),
                )
                .unwrap();
            }
        }
        fs::create_dir_all(root.join("src/users")).unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { Dto } from '../level5/dto';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: Dto) {}
}
"#,
        )
        .unwrap();

        let mut resolver = DtoResolver::new(&root, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("Dto", Path::new("src/users/users.controller.ts"));
        // Should be Unknown due to depth limit
        assert_eq!(resolution, DtoResolution::Unknown);
        // Should have a warning diagnostic
        assert!(
            resolver
                .diagnostics
                .iter()
                .any(|d| d.message.contains("re-export depth"))
        );
    }

    #[test]
    fn detects_circular_imports() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/a")).unwrap();
        fs::create_dir_all(root.join("src/b")).unwrap();
        fs::create_dir_all(root.join("src/users")).unwrap();
        // Create a circular re-export: a.ts -> b.ts -> a.ts
        fs::write(root.join("src/a/a.ts"), "export { A } from '../b/b';").unwrap();
        fs::write(root.join("src/b/b.ts"), "export { A } from '../a/a';").unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { A } from '../a/a';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: A) {}
}
"#,
        )
        .unwrap();

        let mut resolver = DtoResolver::new(&root, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("A", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Unknown);
        assert!(
            resolver
                .diagnostics
                .iter()
                .any(|d| d.message.contains("circular"))
        );
    }

    #[test]
    fn creates_dto_schema_from_create_zod_dto() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/create-user.schema.ts"),
            r#"
import { z } from 'zod';
export const CreateUserSchema = z.object({ name: z.string(), email: z.string() });
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/users/dto/create-user.dto.ts"),
            r#"
import { createZodDto } from 'nestjs-zod';
import { CreateUserSchema } from './create-user.schema';
export class CreateUserDto extends createZodDto(CreateUserSchema) {}
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { CreateUserDto } from './dto/create-user.dto';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: CreateUserDto) {}
}
"#,
        )
        .unwrap();

        let mut resolver = DtoResolver::new(&root, Path::new("src"), &config, &matcher);
        let resolution =
            resolver.resolve_type("CreateUserDto", Path::new("src/users/users.controller.ts"));
        assert_eq!(
            resolution,
            DtoResolution::ZodFields(vec!["name".into(), "email".into()])
        );
    }
}
