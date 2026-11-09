//! Cross-file DTO resolution module.
//!
//! This module resolves DTO types (class, interface, type alias, Zod schema)
//! from imports in controller files, following import chains up to a bounded
//! depth. It integrates with the existing analysis pipeline to provide
//! `BodyShape::Fields` for cross-file DTOs.

pub mod containment;
pub mod imports;
pub mod limits;
pub mod module;
pub mod packages;
pub mod provider;
pub mod tsconfig;
pub mod zod;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::diag::{Diagnostic, Severity};
use crate::discovery::Matcher;
use crate::resolve::containment::DependencyPathError;
use crate::resolve::imports::{ModuleImportsExports, extract_imports_exports};
use crate::resolve::limits::*;
use crate::resolve::module::{ModuleResolveError, resolve_module};
use crate::resolve::packages::PackageImports;
use crate::resolve::provider::{DiskProvider, SourceProvider};
use crate::resolve::tsconfig::TsConfig;
use crate::resolve::zod::analyze_zod_program;
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

/// Run-scoped state for cross-file DTO resolution: the module cache, the
/// resource budgets, and diagnostics.
///
/// One context per analysis run. Every Controller file and every route
/// resolution within the run shares it, so a dependency file is read and
/// parsed once no matter how many Controllers import it. The context is never
/// persisted across runs (separate CLI executions, watch batches, and
/// analysis snapshots each start fresh, which is also what keeps the cache
/// from ever going stale).
pub struct ResolverContext {
    /// Where dependency content comes from (working tree or Git index).
    provider: Box<dyn SourceProvider>,
    /// Parsed tsconfig (if any).
    tsconfig: Option<TsConfig>,
    /// `package.json` `imports` subpath mappings (if any), read through the
    /// provider so staged analysis sees the staged package.json.
    package_imports: Option<PackageImports>,
    /// Cache of analyzed modules keyed by normalized project-relative path
    /// (the exact paths `module::resolve_module` produces: explicit `.ts`
    /// files or an `index.ts`, never directories or bare specifiers).
    module_cache: HashMap<PathBuf, ModuleAnalysis>,
    /// Dependency graph of this run: controller file -> the dependency module
    /// paths its cross-file resolutions touched (including modules that were
    /// resolved but unreadable, so that recreating them is observable).
    /// Harvested by watch mode to invalidate dependent controllers.
    dependency_edges: HashMap<PathBuf, BTreeSet<PathBuf>>,
    /// Controllers whose dependency set is *incomplete*: an import binding was
    /// found but its specifier resolved to no readable module (deleted or
    /// renamed dependency). The watch merge must keep the previous edges for
    /// these instead of replacing them with the partial set, so that
    /// recreating the dependency is still observable.
    edge_uncertain: BTreeSet<PathBuf>,
    /// The controller file whose resolution is in progress; attribution for
    /// [`Self::dependency_edges`].
    current_attribution: Option<PathBuf>,
    /// Import/export edges established by specifier resolution.
    import_edge_count: usize,
    /// Total bytes of resolved dependency sources.
    total_dependency_bytes: usize,
    /// Number of dependency files resolved.
    dependency_file_count: usize,
    /// Number of DTO declarations extracted.
    dto_declaration_count: usize,
    /// Diagnostics collected during resolution.
    diagnostics: Vec<Diagnostic>,
}

impl ResolverContext {
    /// Create the context for one analysis run over `project_root`, reading
    /// dependencies from the working tree.
    pub fn new(project_root: &Path) -> Self {
        let tsconfig = TsConfig::load(project_root);
        let (provider, warning) = DiskProvider::new(project_root);
        let diagnostics = warning
            .map(|message| {
                // Canonicalization failed: dependency reads are rejected
                // conservatively (cross-file resolution degrades to
                // BodyShape::Unknown, never to a guessed body).
                Diagnostic::new(Severity::Warning, Some(project_root.to_path_buf()), message)
            })
            .into_iter()
            .collect();
        Self::with_parts(Box::new(provider), tsconfig, diagnostics)
    }

    /// Create a context with an explicit dependency source — the Git-index
    /// provider for staged analysis — and an explicit tsconfig (the
    /// index-derived one, when the staged snapshot has one).
    pub fn with_parts(
        provider: Box<dyn SourceProvider>,
        tsconfig: Option<TsConfig>,
        initial_diagnostics: Vec<Diagnostic>,
    ) -> Self {
        // `imports` subpath mappings come through the same boundary as every
        // other dependency read: staged analysis sees the staged package.json.
        let package_imports = provider
            .read(Path::new("package.json"))
            .ok()
            .and_then(|text| PackageImports::parse(&text));
        Self {
            provider,
            tsconfig,
            package_imports,
            module_cache: HashMap::new(),
            dependency_edges: HashMap::new(),
            edge_uncertain: BTreeSet::new(),
            current_attribution: None,
            import_edge_count: 0,
            total_dependency_bytes: 0,
            dependency_file_count: 0,
            dto_declaration_count: 0,
            diagnostics: initial_diagnostics,
        }
    }

    /// All diagnostics collected so far in this run.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Take the dependency graph recorded so far, leaving the context without
    /// it: the controller -> dependency-module map, and the set of controllers
    /// whose map entry is known to be incomplete. Watch mode merges both into
    /// its persistent reverse index after each analysis run.
    pub fn take_dependency_graph(
        &mut self,
    ) -> (HashMap<PathBuf, BTreeSet<PathBuf>>, BTreeSet<PathBuf>) {
        (
            std::mem::take(&mut self.dependency_edges),
            std::mem::take(&mut self.edge_uncertain),
        )
    }
}

/// Per-controller-file resolver: borrows the run-scoped [`ResolverContext`]
/// and holds only per-resolution state (the cycle detection stack). Cheap to
/// create per file or per route.
pub struct DtoResolver<'ctx> {
    /// Shared run state.
    ctx: &'ctx mut ResolverContext,
    /// Source directory prefix (e.g., "src").
    source_prefix: &'ctx Path,
    /// Configuration.
    config: &'ctx Config,
    /// Discovery matcher for exclusion checks.
    matcher: &'ctx Matcher,
    /// Resolution stack for cycle detection: (module_path, symbol_name) pairs
    /// currently being resolved. A cycle is detected when the same pair appears twice.
    resolution_stack: Vec<(PathBuf, String)>,
}

/// Analysis of one TypeScript module for DTO resolution.
///
/// Holds only compact AST-derived data; the parsed AST and the source text
/// are dropped as soon as the module's analysis has been built.
#[derive(Debug, Clone)]
struct ModuleAnalysis {
    /// Imports and exports from the module.
    imports_exports: ModuleImportsExports,
    /// Zod schemas defined in this module.
    zod_schemas: HashMap<String, crate::resolve::zod::ZodSchema>,
    /// Class/interface/type alias property names (same-file DTO extraction).
    local_dtos: HashMap<String, Vec<String>>,
}

impl<'ctx> DtoResolver<'ctx> {
    /// Create a resolver for one controller file over the shared run context.
    pub fn new(
        ctx: &'ctx mut ResolverContext,
        source_prefix: &'ctx Path,
        config: &'ctx Config,
        matcher: &'ctx Matcher,
    ) -> Self {
        Self {
            ctx,
            source_prefix,
            config,
            matcher,
            resolution_stack: Vec::new(),
        }
    }

    /// Resolve a type name (from a @Body parameter) to its properties.
    /// Returns DtoResolution with the properties or Unknown/Uncertain.
    pub fn resolve_type(&mut self, type_name: &str, from_file: &Path) -> DtoResolution {
        // First check same-file DTOs (already extracted in extract.rs)
        // The caller should pass same-file DTOs separately; this only does cross-file.

        // Attribute every dependency this resolution touches to `from_file`
        // (watch mode invalidates it when one of them changes).
        self.ctx.current_attribution = Some(from_file.to_path_buf());

        // Find the import that brings `type_name` into scope
        let Some((imported_name, specifier)) = self.find_import_binding(type_name, from_file)
        else {
            return DtoResolution::Unknown;
        };

        // Resolve the module specifier
        let Some(module_path) = self.resolve_imported_module(&imported_name, &specifier, from_file)
        else {
            // The import binding exists but its specifier led nowhere (deleted,
            // renamed, excluded, or unreadable dependency): the recorded edge
            // set for this controller is incomplete.
            self.ctx.edge_uncertain.insert(from_file.to_path_buf());
            return DtoResolution::Unknown;
        };

        // Look up the exported symbol (analyzes module internally)
        self.lookup_export(&imported_name, &module_path, 0)
    }

    /// Get all diagnostics collected during resolution (the whole run).
    pub fn diagnostics(&self) -> &[Diagnostic] {
        self.ctx.diagnostics()
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
    ///
    /// Each specifier that resolves to a project module consumes one import
    /// edge from [`MAX_IMPORT_EDGES`]; the edge that would exceed the budget
    /// is not followed and resolution degrades to `Unknown`.
    fn resolve_imported_module(
        &mut self,
        _imported_name: &str,
        specifier: &str,
        from_file: &Path,
    ) -> Option<PathBuf> {
        if self.ctx.import_edge_count >= MAX_IMPORT_EDGES {
            self.add_diagnostic(
                Severity::Warning,
                from_file,
                "import edge limit reached; stopping cross-file resolution",
            );
            return None;
        }
        let env = module::ModuleEnv {
            source_prefix: self.source_prefix,
            config: self.config,
            matcher: self.matcher,
            tsconfig: self.ctx.tsconfig.as_ref(),
            provider: self.ctx.provider.as_ref(),
            package_imports: self.ctx.package_imports.as_ref(),
        };
        let resolved = match resolve_module(specifier, from_file, &env) {
            Ok(Some(path)) => Some(path),
            Ok(None) => None,
            Err(ModuleResolveError::EscapesProject) => {
                // Path escapes project - diagnostic but not fatal
                None
            }
            Err(ModuleResolveError::Io) => None,
        };
        if let Some(path) = &resolved {
            self.ctx.import_edge_count = self.ctx.import_edge_count.saturating_add(1);
            // Record the dependency edge even when the module later turns out
            // to be unreadable: recreating it must be observable (watch mode).
            if let Some(who) = &self.ctx.current_attribution {
                self.ctx
                    .dependency_edges
                    .entry(who.clone())
                    .or_default()
                    .insert(path.clone());
            }
        }
        resolved
    }

    /// Analyze a module (parse, extract imports/exports, Zod schemas, local DTOs).
    ///
    /// Before any filesystem read the project-relative path is validated by
    /// [`validated_dependency_path`]: no symlink components, regular file
    /// final target, contained in the canonical project root. Rejected or
    /// unreadable dependencies yield no module analysis (`None`), which
    /// callers surface as `DtoResolution::Unknown`.
    ///
    /// Budget boundary semantics are consistent across all limits: the
    /// operation that would exceed a budget is rejected in full; work already
    /// completed within budget stays cached and valid. A module is either
    /// analyzed completely (all counters committed) or not at all.
    fn analyze_module(&mut self, module_path: &Path) -> Option<&ModuleAnalysis> {
        // Check cache first
        if self.ctx.module_cache.contains_key(module_path) {
            return self.ctx.module_cache.get(module_path);
        }

        // Check limits
        if self.ctx.dependency_file_count >= MAX_DEPENDENCY_FILES {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                "dependency file limit reached; stopping cross-file resolution",
            );
            return None;
        }

        // Validated read through the source provider: containment for the
        // working tree (canonical root, symlink-free components, regular
        // file), mode checks for index blobs. Rejections are conservative
        // (no partial results, no reads outside the boundary).
        let source = match self.ctx.provider.read(module_path) {
            Ok(s) => s,
            Err(DependencyPathError::NotFound(_)) => {
                self.add_diagnostic(Severity::Warning, module_path, "dependency file not found");
                return None;
            }
            Err(DependencyPathError::Io(_)) => {
                self.add_diagnostic(
                    Severity::Warning,
                    module_path,
                    "cannot read dependency file",
                );
                return None;
            }
            Err(reason) => {
                self.add_diagnostic(
                    Severity::Warning,
                    module_path,
                    format!("rejected dependency path: {reason}"),
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

        // Check total bytes limit (checked: the aggregate can only grow).
        let would_total = match self.ctx.total_dependency_bytes.checked_add(source.len()) {
            Some(total) => total,
            None => {
                self.add_diagnostic(
                    Severity::Warning,
                    module_path,
                    "total dependency bytes limit reached; stopping",
                );
                return None;
            }
        };
        if would_total > MAX_TOTAL_DEPENDENCY_BYTES {
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

        // Single parse: imports/exports, Zod schemas and local DTOs all read
        // the same program; nothing re-parses the source.
        let zod_schemas = analyze_zod_program(&parsed.program);

        // Extract same-file class/interface properties (like dto_properties in extract.rs)
        let local_dtos = extract_local_dtos(&parsed.program);

        // Declaration budget: a module that would push the run past
        // MAX_DTO_DECLARATIONS is rejected whole so no partially tracked
        // declaration set is ever published.
        let declarations = local_dtos.len() + zod_schemas.len();
        let would_declare = match self.ctx.dto_declaration_count.checked_add(declarations) {
            Some(total) => total,
            None => {
                self.add_diagnostic(
                    Severity::Warning,
                    module_path,
                    "DTO declaration limit reached",
                );
                return None;
            }
        };
        if would_declare > MAX_DTO_DECLARATIONS {
            self.add_diagnostic(
                Severity::Warning,
                module_path,
                "DTO declaration limit reached; stopping cross-file resolution",
            );
            return None;
        }

        let analysis = ModuleAnalysis {
            imports_exports,
            zod_schemas,
            local_dtos,
        };

        self.ctx.total_dependency_bytes = would_total;
        self.ctx.dependency_file_count = self.ctx.dependency_file_count.saturating_add(1);
        self.ctx.dto_declaration_count = would_declare;

        self.ctx
            .module_cache
            .insert(module_path.to_path_buf(), analysis);
        self.ctx.module_cache.get(module_path)
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
                // A dangling createZodDto import: the edge set is incomplete.
                if let Some(who) = &self.ctx.current_attribution {
                    self.ctx.edge_uncertain.insert(who.clone());
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
            // A dangling re-export target: the edge set is incomplete.
            if let Some(who) = &self.ctx.current_attribution {
                self.ctx.edge_uncertain.insert(who.clone());
            }
        }
        self.resolution_stack.pop();
        DtoResolution::Unknown
    }

    fn add_diagnostic(&mut self, severity: Severity, path: &Path, message: impl Into<String>) {
        self.ctx.diagnostics.push(Diagnostic::new(
            severity,
            Some(path.to_path_buf()),
            message.into(),
        ));
    }
}

/// Extract class/interface property names from a program: exported and plain
/// top-level declarations, source order, computed names skipped, at most
/// [`MAX_DTO_PROPERTIES`] per declaration.
fn extract_local_dtos(program: &oxc_ast::ast::Program<'_>) -> HashMap<String, Vec<String>> {
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

/// Seed the run cache with a module that was already parsed by the extractor
/// (the controller file itself): its AST-derived metadata is computed from the
/// program at hand instead of re-reading and re-parsing the file from disk.
///
/// Seeding performs no I/O, so it consumes no dependency budget; the
/// controller file is already bounded by the extraction-time source cap, and
/// its declarations still count toward the run's declaration budget. A module
/// already in the cache (analyzed as a dependency of an earlier controller)
/// is left untouched.
pub fn seed_module_from_program(
    ctx: &mut ResolverContext,
    module_path: &Path,
    program: &oxc_ast::ast::Program<'_>,
) {
    if ctx.module_cache.contains_key(module_path) {
        return;
    }
    // The controller was analyzed in this run: make sure its edge entry
    // exists (possibly empty) so a watch harvest replaces any stale edges.
    ctx.dependency_edges
        .entry(module_path.to_path_buf())
        .or_default();
    let analysis = ModuleAnalysis {
        imports_exports: extract_imports_exports(program),
        zod_schemas: analyze_zod_program(program),
        local_dtos: extract_local_dtos(program),
    };
    let declarations = analysis.local_dtos.len() + analysis.zod_schemas.len();
    ctx.dto_declaration_count = ctx.dto_declaration_count.saturating_add(declarations);
    ctx.module_cache.insert(module_path.to_path_buf(), analysis);
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
///
/// Called from `nest/extract.rs` after same-file DTO extraction, with the
/// run-scoped context so repeated resolutions share the module cache and
/// budgets. Only the diagnostics emitted by this call are returned; the
/// context keeps accumulating for the rest of the run.
pub fn resolve_cross_file_dtos(
    ctx: &mut ResolverContext,
    source_prefix: &Path,
    config: &Config,
    matcher: &Matcher,
    controller_file: &Path,
    body_params: &[RouteParameter],
    same_file_dtos: &HashMap<String, Vec<String>>,
) -> CrossFileDtoResolutions {
    let diagnostics_before = ctx.diagnostics.len();
    let mut resolver = DtoResolver::new(ctx, source_prefix, config, matcher);
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
                resolver.ctx.diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    Some(controller_file.to_path_buf()),
                    format!("Zod schema resolution for {type_name} is uncertain due to unsupported transformations; using available properties"),
                ));
            }
            _ => {}
        }
    }

    let diagnostics = resolver
        .ctx
        .diagnostics
        .drain(diagnostics_before..)
        .collect();
    CrossFileDtoResolutions {
        resolutions,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
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
            bootstrap: None,
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

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
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

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
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

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
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

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
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

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("Dto", Path::new("src/users/users.controller.ts"));
        // Should be Unknown due to depth limit
        assert_eq!(resolution, DtoResolution::Unknown);
        // Should have a warning diagnostic
        assert!(
            resolver
                .diagnostics()
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

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("A", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Unknown);
        assert!(
            resolver
                .diagnostics()
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

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution =
            resolver.resolve_type("CreateUserDto", Path::new("src/users/users.controller.ts"));
        assert_eq!(
            resolution,
            DtoResolution::ZodFields(vec!["name".into(), "email".into()])
        );
    }

    #[test]
    fn reexport_depth_at_limit_resolves() {
        let (_dir, root, config, matcher) = setup_project();
        // Exactly MAX_REEXPORT_HOPS re-export modules: level0 declares, each
        // level above re-exports. The declaration is reached at depth 4,
        // which the budget still allows (only depth > 4 is rejected).
        for i in 0..5 {
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
import { Dto } from '../level4/dto';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: Dto) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("Dto", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Fields(vec!["a".into()]));
    }

    #[test]
    fn missing_dependency_resolves_after_creation() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { LateDto } from './dto/late';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: LateDto) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution =
            resolver.resolve_type("LateDto", Path::new("src/users/users.controller.ts"));
        // A specifier that matches no project file resolves silently to
        // None (indistinguishable from an external package) -> Unknown.
        assert_eq!(resolution, DtoResolution::Unknown);

        // Failures are not cached: creating the file makes the next
        // resolution of the same specifier succeed.
        fs::write(
            root.join("src/users/dto/late.ts"),
            "export class LateDto { id: string; }",
        )
        .unwrap();
        let resolution =
            resolver.resolve_type("LateDto", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Fields(vec!["id".into()]));
    }

    #[test]
    fn rejects_directory_dependency() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/weird.ts")).unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { X } from './weird.ts';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: X) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("X", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Unknown);
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("not a regular file"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_dependency_target() {
        let (_dir, root, config, matcher) = setup_project();
        let outside = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            outside.path().join("leaked.ts"),
            "export class X { a: string; }",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("leaked.ts"),
            root.join("src/users/dto/leaked.ts"),
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { X } from './dto/leaked';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: X) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("X", Path::new("src/users/users.controller.ts"));
        // The dependency is never read: the result is Unknown, not the
        // properties of the out-of-root file.
        assert_eq!(resolution, DtoResolution::Unknown);
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("symlinked path component"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_intermediate_directory() {
        let (_dir, root, config, matcher) = setup_project();
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("x.ts"), "export class X { a: string; }").unwrap();
        fs::create_dir_all(root.join("src/users")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { X } from './dto/x';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: X) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("X", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Unknown);
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("symlinked path component"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_loop_dependency() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        std::os::unix::fs::symlink(
            root.join("src/users/dto/b.ts"),
            root.join("src/users/dto/a.ts"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            root.join("src/users/dto/a.ts"),
            root.join("src/users/dto/b.ts"),
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { X } from './dto/a';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: X) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("X", Path::new("src/users/users.controller.ts"));
        // A loop is just a symlink chain: rejected without being followed.
        assert_eq!(resolution, DtoResolution::Unknown);
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("symlinked path component"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_special_file_dependency() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users")).unwrap();
        // A Unix socket is a special file; reading it would block.
        let _listener =
            std::os::unix::net::UnixListener::bind(root.join("src/users/sock.ts")).unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { X } from './sock.ts';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: X) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("X", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Unknown);
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("not a regular file"))
        );
    }

    #[test]
    fn dependency_file_limit_boundary() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/d")).unwrap();
        for i in 0..MAX_DEPENDENCY_FILES {
            fs::write(
                root.join("src/d").join(format!("m{i}.ts")),
                format!("export class C{i} {{}}"),
            )
            .unwrap();
        }
        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        for i in 0..MAX_DEPENDENCY_FILES {
            let path = PathBuf::from(format!("src/d/m{i}.ts"));
            assert!(
                resolver.analyze_module(&path).is_some(),
                "file {i} is within the file budget and must analyze"
            );
        }
        // The file that would exceed the budget is rejected in full.
        fs::write(root.join("src/d/extra.ts"), "export class Extra {}").unwrap();
        assert!(
            resolver
                .analyze_module(Path::new("src/d/extra.ts"))
                .is_none()
        );
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("dependency file limit"))
        );
    }

    #[test]
    fn per_file_source_limit_boundary() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/d")).unwrap();
        // Exactly MAX_SOURCE_BYTES: accepted (the check is strictly greater).
        let at_limit = format!("//{}\n", "x".repeat(MAX_SOURCE_BYTES - 3));
        fs::write(root.join("src/d/at.ts"), &at_limit).unwrap();
        // One byte beyond: rejected before parsing.
        let beyond = format!("//{}\n", "x".repeat(MAX_SOURCE_BYTES - 2));
        fs::write(root.join("src/d/beyond.ts"), &beyond).unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        assert!(resolver.analyze_module(Path::new("src/d/at.ts")).is_some());
        assert!(
            resolver
                .analyze_module(Path::new("src/d/beyond.ts"))
                .is_none()
        );
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("too large"))
        );
    }

    #[test]
    fn total_dependency_bytes_boundary() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/d")).unwrap();
        // MAX_TOTAL_DEPENDENCY_BYTES is exactly 16 files of MAX_SOURCE_BYTES.
        let full = format!("//{}\n", "x".repeat(MAX_SOURCE_BYTES - 3));
        for i in 0..16 {
            fs::write(root.join("src/d").join(format!("f{i}.ts")), &full).unwrap();
        }
        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        for i in 0..16 {
            let path = PathBuf::from(format!("src/d/f{i}.ts"));
            assert!(
                resolver.analyze_module(&path).is_some(),
                "file {i} fills the byte budget exactly and must analyze"
            );
        }
        // Aggregate is at exactly MAX_TOTAL_DEPENDENCY_BYTES: any further
        // file, however small, is rejected.
        fs::write(root.join("src/d/tiny.ts"), "//\n").unwrap();
        assert!(
            resolver
                .analyze_module(Path::new("src/d/tiny.ts"))
                .is_none()
        );
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("total dependency bytes limit"))
        );
    }

    #[test]
    fn dto_declaration_limit_boundary() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/d")).unwrap();
        let schemas = |count: usize| -> String {
            let mut source = String::from("import { z } from 'zod';\n");
            for i in 0..count {
                source.push_str(&format!(
                    "export const S{i} = z.object({{ a: z.string() }});\n"
                ));
            }
            source
        };
        // Two modules of 512 schemas fill the 1024-declaration budget exactly.
        fs::write(root.join("src/d/half0.ts"), schemas(512)).unwrap();
        fs::write(root.join("src/d/half1.ts"), schemas(512)).unwrap();
        // The module that would exceed the budget is rejected whole.
        fs::write(root.join("src/d/one.ts"), schemas(1)).unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        assert!(
            resolver
                .analyze_module(Path::new("src/d/half0.ts"))
                .is_some()
        );
        assert!(
            resolver
                .analyze_module(Path::new("src/d/half1.ts"))
                .is_some()
        );
        assert!(resolver.analyze_module(Path::new("src/d/one.ts")).is_none());
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("DTO declaration limit"))
        );
        // The rejected module's declarations were not partially committed:
        // the budget stays exactly at the limit.
        assert_eq!(resolver.ctx.dto_declaration_count, MAX_DTO_DECLARATIONS);
    }

    #[test]
    fn import_edge_limit_boundary() {
        let (_dir, root, config, matcher) = setup_project();
        // Relative to src/users/ (the importing file's directory), "./d/x"
        // must name src/users/d/x.ts.
        fs::create_dir_all(root.join("src/users/d")).unwrap();
        fs::write(root.join("src/users/d/x.ts"), "export class X {}").unwrap();
        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let from = Path::new("src/users/users.controller.ts");
        for _ in 0..MAX_IMPORT_EDGES {
            assert!(
                resolver
                    .resolve_imported_module("X", "./d/x", from)
                    .is_some()
            );
        }
        // The edge that would exceed the budget is not followed.
        assert!(
            resolver
                .resolve_imported_module("X", "./d/x", from)
                .is_none()
        );
        assert!(
            resolver
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("import edge limit"))
        );
    }

    #[test]
    fn resolves_package_imports_subpaths() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/shared")).unwrap();
        fs::create_dir_all(root.join("src/users")).unwrap();
        fs::write(
            root.join("package.json"),
            r##"{ "imports": { "#src/*": "./src/*" } }"##,
        )
        .unwrap();
        fs::write(
            root.join("src/shared/pager.dto.ts"),
            "export class Pager { page: string; }",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { Pager } from '#src/shared/pager.dto.js';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: Pager) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("Pager", Path::new("src/users/users.controller.ts"));
        assert_eq!(
            resolution,
            DtoResolution::Fields(vec!["page".into()]),
            "the #src subpath maps through package.json imports and the NodeNext .js extension"
        );
    }

    #[test]
    fn an_external_package_imports_target_is_not_resolved() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users")).unwrap();
        fs::write(
            root.join("package.json"),
            r##"{ "imports": { "#lib/*": "some-package/*" } }"##,
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { Thing } from '#lib/thing';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: Thing) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
        let resolution = resolver.resolve_type("Thing", Path::new("src/users/users.controller.ts"));
        assert_eq!(resolution, DtoResolution::Unknown);
    }

    // ------------------------------------------------------------ properties

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Resolution is a pure function of the source tree: the same tree
        /// gives the same result, for class DTOs, re-export chains and
        /// cycles alike, and always terminates.
        #[test]
        fn resolution_is_deterministic_and_terminates(
            hops in 0usize..8,
            cycle in proptest::bool::ANY,
            field_count in 1usize..5,
            shared in "[a-z]{1,8}",
        ) {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let fields: Vec<String> = (0..field_count)
                .map(|i| format!("f{i}: string;"))
                .collect();
            // A chain of `hops` re-export files; optionally the last one
            // points back at the first (a cycle).
            fs::create_dir_all(root.join("src/d")).unwrap();
            fs::write(
                root.join("src/d/leaf.ts"),
                format!("export class {} {{ {} }}", shared, fields.join(" ")),
            )
            .unwrap();
            for i in 0..hops {
                let from = if i == 0 { "./d/leaf".to_owned() } else { format!("./c{}", i - 1) };
                let back = if cycle && i + 1 == hops {
                    format!(" export {{ {shared} }} from './c0';")
                } else {
                    String::new()
                };
                fs::write(
                    root.join(format!("src/c{i}.ts")),
                    format!("export {{ {shared} }} from '{from}';{back}"),
                )
                .unwrap();
            }
            fs::create_dir_all(root.join("src/users")).unwrap();
            let specifier = if hops == 0 { "./d/leaf".to_owned() } else { format!("../c{}", hops - 1) };
            fs::write(
                root.join("src/users/users.controller.ts"),
                format!(
                    "import {{ {shared} }} from '{specifier}';
@Controller('users') export class U {{ @Post() c(@Body() dto: {shared}) {{}} }}
"
                ),
            )
            .unwrap();

            let config = Config {
                source: PathBuf::from("src"),
                bruno: PathBuf::from("bruno"),
                base_url: "{{baseUrl}}".into(),
                include: vec!["**/*.controller.ts".into()],
                exclude: vec![],
                bootstrap: None,
            };
            let matcher = Matcher::new(&config).unwrap();
            let resolve = || {
                let mut ctx = ResolverContext::new(root);
                let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
                resolver.resolve_type(&shared, Path::new("src/users/users.controller.ts"))
            };
            let first = resolve();
            let second = resolve();
            prop_assert_eq!(first, second);
        }

        /// Containment: no project-relative path input ever produces a read
        /// outside the canonical project root — the validator either rejects
        /// or names a path under the root.
        #[test]
        fn containment_never_escapes_the_root(
            segments in proptest::collection::vec("[a-zA-Z0-9_.]{0,6}", 0..6),
            escape in proptest::bool::ANY,
        ) {
            use crate::resolve::containment::validated_dependency_path;
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(root.join("src/real.ts"), "export class R {}").unwrap();

            let mut relative = PathBuf::new();
            for segment in &segments {
                relative.push(segment.as_str());
            }
            if escape {
                relative = Path::new("..").join(&relative);
            }

            // The validator either rejects the path or returns something
            // strictly inside the canonical root.
            if let Ok(validated) = validated_dependency_path(&root, &relative) {
                prop_assert!(validated.starts_with(&root));
            }
        }
    }

    #[test]
    fn run_scoped_context_shares_modules_across_controllers() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/shared")).unwrap();
        fs::create_dir_all(root.join("src/a")).unwrap();
        fs::create_dir_all(root.join("src/b")).unwrap();
        fs::write(
            root.join("src/shared/shared.dto.ts"),
            "export class Shared { x: string; }",
        )
        .unwrap();
        for dir in ["a", "b"] {
            fs::write(
                root.join(format!("src/{dir}/{dir}.controller.ts")),
                format!(
                    "import {{ Shared }} from '../shared/shared.dto';\n\
                     export class {dir}Uses {{}}\n"
                ),
            )
            .unwrap();
        }

        let mut ctx = ResolverContext::new(&root);
        let first = {
            let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
            resolver.resolve_type("Shared", Path::new("src/a/a.controller.ts"))
        };
        assert_eq!(first, DtoResolution::Fields(vec!["x".into()]));
        let files_after_first = ctx.dependency_file_count;

        // A second controller over the SAME run context: the shared DTO
        // module is served from the cache; only the new controller file is
        // read.
        let second = {
            let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
            resolver.resolve_type("Shared", Path::new("src/b/b.controller.ts"))
        };
        assert_eq!(second, DtoResolution::Fields(vec!["x".into()]));
        assert_eq!(ctx.dependency_file_count, files_after_first + 1);
        assert_eq!(ctx.module_cache.len(), 3);
    }

    #[test]
    fn cache_does_not_leak_across_runs() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/evolving.ts"),
            "export class Evolving { a: string; }",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { Evolving } from './dto/evolving';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: Evolving) {}
}
"#,
        )
        .unwrap();

        let mut first_run = ResolverContext::new(&root);
        let resolved = {
            let mut resolver =
                DtoResolver::new(&mut first_run, Path::new("src"), &config, &matcher);
            resolver.resolve_type("Evolving", Path::new("src/users/users.controller.ts"))
        };
        assert_eq!(resolved, DtoResolution::Fields(vec!["a".into()]));

        // The dependency changes on disk. A new run gets a fresh context and
        // must see the new shape, never the cached one.
        fs::write(
            root.join("src/users/dto/evolving.ts"),
            "export class Evolving { a: string; b: number; }",
        )
        .unwrap();
        let mut second_run = ResolverContext::new(&root);
        let resolved = {
            let mut resolver =
                DtoResolver::new(&mut second_run, Path::new("src"), &config, &matcher);
            resolver.resolve_type("Evolving", Path::new("src/users/users.controller.ts"))
        };
        assert_eq!(
            resolved,
            DtoResolution::Fields(vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn malformed_dependency_retries_after_repair() {
        let (_dir, root, config, matcher) = setup_project();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/broken.ts"),
            "export classs Broken {",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            r#"
import { Broken } from './dto/broken';
@Controller('users')
export class UsersController {
  @Post() create(@Body() dto: Broken) {}
}
"#,
        )
        .unwrap();

        let mut ctx = ResolverContext::new(&root);
        let resolved = {
            let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
            resolver.resolve_type("Broken", Path::new("src/users/users.controller.ts"))
        };
        assert_eq!(resolved, DtoResolution::Unknown);
        assert!(
            ctx.diagnostics()
                .iter()
                .any(|d| d.message.contains("failed to parse"))
        );

        // A malformed dependency is not cached: repairing the file makes the
        // next resolution in the same run succeed.
        fs::write(
            root.join("src/users/dto/broken.ts"),
            "export class Broken { id: string; }",
        )
        .unwrap();
        let resolved = {
            let mut resolver = DtoResolver::new(&mut ctx, Path::new("src"), &config, &matcher);
            resolver.resolve_type("Broken", Path::new("src/users/users.controller.ts"))
        };
        assert_eq!(resolved, DtoResolution::Fields(vec!["id".into()]));
    }
}
