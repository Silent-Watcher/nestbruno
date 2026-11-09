//! Module resolution: mapping import specifiers to project-relative file paths.
//!
//! Supports:
//! - Relative imports: `./foo`, `../bar`, `./dir` (directory -> `./dir/index.ts`)
//! - tsconfig `paths` with wildcards and exact matches
//! - tsconfig `baseUrl`
//! - Implicit `.ts` extension and `index.ts` for directories
//! - NodeNext-style emitted extensions (`./x.js` -> `./x.ts`, `.mjs` ->
//!   `.mts`, `.cjs` -> `.cts`), as required by `moduleResolution: "NodeNext"`
//! - Project-root containment and exclusion checks

use std::path::{Component, Path, PathBuf};

use crate::config::Config;
use crate::discovery::Matcher;
use crate::resolve::packages::PackageImports;
use crate::resolve::provider::SourceProvider;
use crate::resolve::tsconfig::TsConfig;

/// Everything a specifier resolution may consult — constant for a whole
/// analysis run.
pub struct ModuleEnv<'a> {
    /// Source directory prefix (e.g., "src").
    pub source_prefix: &'a Path,
    /// Configuration.
    pub config: &'a Config,
    /// Discovery matcher for exclusion checks.
    pub matcher: &'a Matcher,
    /// Parsed tsconfig (if any).
    pub tsconfig: Option<&'a TsConfig>,
    /// Where dependency content comes from.
    pub provider: &'a dyn SourceProvider,
    /// `package.json` `imports` subpath mappings (if any).
    pub package_imports: Option<&'a PackageImports>,
}

/// Resolves a module specifier to a project-relative path.
pub fn resolve_module(
    specifier: &str,
    from_file: &Path,
    env: &ModuleEnv<'_>,
) -> Result<Option<PathBuf>, ModuleResolveError> {
    let from_dir = from_file.parent().unwrap_or(Path::new(""));

    if specifier.starts_with("./") || specifier.starts_with("../") {
        // Relative import: resolve relative to the importing file's directory
        resolve_relative(specifier, from_dir, env)
    } else {
        // Bare specifier: try tsconfig paths, then baseUrl, then `imports`
        // subpaths, then node_modules (deferred)
        resolve_bare(specifier, env)
    }
}

/// Resolve a relative specifier (starting with ./ or ../).
fn resolve_relative(
    specifier: &str,
    from_dir: &Path,
    env: &ModuleEnv<'_>,
) -> Result<Option<PathBuf>, ModuleResolveError> {
    // Build the candidate path relative to project root.
    // from_dir is project-relative and already includes source_prefix
    // (e.g., "src/users" when source_prefix is "src").
    let mut candidate = from_dir.to_path_buf();

    // Apply the specifier path components, resolving `.` and `..` lexically.
    for comp in specifier.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if !candidate.pop() {
                    return Err(ModuleResolveError::EscapesProject);
                }
            }
            part => candidate.push(part),
        }
    }

    // Candidate is now project-relative. Check containment and exclusions.
    if !is_within_project(&candidate) {
        return Err(ModuleResolveError::EscapesProject);
    }
    if is_excluded(&candidate, env.source_prefix, env.matcher) {
        return Ok(None);
    }

    // Try file candidates: .ts, /index.ts
    try_file_candidates(&candidate, env.provider)
}

/// Resolve a bare specifier (not starting with ./ or ../).
fn resolve_bare(
    specifier: &str,
    env: &ModuleEnv<'_>,
) -> Result<Option<PathBuf>, ModuleResolveError> {
    // Try tsconfig paths first
    if let Some(tsconfig) = env.tsconfig
        // The tsconfig's paths are relative to baseUrl (if set) or tsconfig dir.
        // We assume tsconfig at project root and paths are project-relative.
        && let Some(resolved) = tsconfig.resolve_bare_specifier(specifier, Path::new(""))
        && is_within_project(&resolved)
        && !is_excluded(&resolved, env.source_prefix, env.matcher)
        && let Some(found) = try_file_candidates(&resolved, env.provider)?
    {
        return Ok(Some(found));
    }

    // Try baseUrl directly
    if let Some(tsconfig) = env.tsconfig
        && let Some(base_url) = &tsconfig.base_url
    {
        let candidate = Path::new(base_url).join(specifier);
        if is_within_project(&candidate)
            && !is_excluded(&candidate, env.source_prefix, env.matcher)
            && let Some(found) = try_file_candidates(&candidate, env.provider)?
        {
            return Ok(Some(found));
        }
    }

    // Node.js `imports` subpaths (`#src/...`): the conservative static
    // subset of package.json `imports` — exact and single-wildcard local
    // targets only; anything else falls through to the node_modules deferral.
    if specifier.starts_with('#')
        && let Some(imports) = env.package_imports
        && let Some(mapped) = imports.resolve(specifier)
        && is_within_project(&mapped)
        && !is_excluded(&mapped, env.source_prefix, env.matcher)
        && let Some(found) = try_file_candidates(&mapped, env.provider)?
    {
        return Ok(Some(found));
    }

    // node_modules resolution is deferred (returns None)
    Ok(None)
}

/// Try file candidates for a directory or file path.
/// Order: NodeNext-emitted extensions mapped to TS sources (`.js` -> `.ts`,
/// `.mjs` -> `.mts`, `.cjs` -> `.cts`), then `<path>.ts`, `<path>/index.ts`,
/// `<path>` (if ends with .ts).
///
/// Existence is asked from the source provider, which answers without
/// following links: a symlinked candidate is selected deliberately so the
/// provider's read can reject it with a precise diagnostic. A bare
/// `exists()` would follow links and silently hide symlink targets and
/// loops behind `ELOOP`.
fn try_file_candidates(
    path: &Path,
    provider: &dyn SourceProvider,
) -> Result<Option<PathBuf>, ModuleResolveError> {
    let path_str = path.to_string_lossy();

    // With `moduleResolution: "NodeNext"`, specifiers must name the emitted
    // file (`./x.js`); the analyzed source is `./x.ts` behind it.
    for mapped in mapped_ts_sources(&path_str) {
        if provider.exists(Path::new(&mapped)) {
            return Ok(Some(PathBuf::from(mapped)));
        }
    }

    // 1. If it ends with .ts, try as-is
    if path_str.ends_with(".ts") && provider.exists(path) {
        return Ok(Some(path.to_path_buf()));
    }

    // 2. Try appending .ts (not replacing extension)
    let with_ts = PathBuf::from(format!("{}.ts", path_str));
    if provider.exists(&with_ts) {
        return Ok(Some(with_ts));
    }

    // 3. Try as directory with index.ts
    let index_ts = path.join("index.ts");
    if provider.exists(&index_ts) {
        return Ok(Some(index_ts));
    }

    Ok(None)
}

/// Map a NodeNext-style emitted extension to the TypeScript sources that may
/// sit behind it, in lookup order: `x.js` -> `x.ts`, `x.mjs` -> `x.mts`,
/// `x.cjs` -> `x.cts`. Empty for any other extension. `x.tsx` behind `x.js`
/// is deliberately not attempted: the pipeline only analyzes `.ts` sources.
fn mapped_ts_sources(path: &str) -> Vec<String> {
    let (stem, source_ext) = if let Some(stem) = path.strip_suffix(".js") {
        (stem, ".ts")
    } else if let Some(stem) = path.strip_suffix(".mjs") {
        (stem, ".mts")
    } else if let Some(stem) = path.strip_suffix(".cjs") {
        (stem, ".cts")
    } else {
        return Vec::new();
    };
    vec![format!("{stem}{source_ext}")]
}

/// Check if a project-relative path is within the project root (no `..` escape).
fn is_within_project(path: &Path) -> bool {
    // The path should not have `..` components (we resolved them lexically).
    // But also check that canonicalization doesn't escape via symlinks.
    // Since we don't have the real filesystem here, just check for `..` components.
    for comp in path.components() {
        if matches!(comp, Component::ParentDir) {
            return false;
        }
    }
    true
}

/// Check if a path is excluded by configuration or always-skipped dirs.
fn is_excluded(path: &Path, source_prefix: &Path, matcher: &Matcher) -> bool {
    // Check if path is under source_prefix
    if let Ok(rel_to_source) = path.strip_prefix(source_prefix) {
        // Apply discovery's exclusion logic
        if matcher.is_pruned_dir(rel_to_source) {
            return true;
        }
        // Also check exclude patterns against the relative path
        // (matcher.exclude.is_match - but we don't have direct access)
        // We'll rely on is_pruned_dir which checks exclude.
    }
    // Check always-skipped dirs anywhere in path
    for comp in path.components() {
        if let Component::Normal(part) = comp
            && (part == ".git" || part == "node_modules")
        {
            return true;
        }
    }
    false
}

/// Error type for module resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleResolveError {
    /// The resolved path would escape the project root.
    EscapesProject,
    /// IO error (not used in this pure function, but for extension).
    Io,
}

impl std::fmt::Display for ModuleResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleResolveError::EscapesProject => f.write_str("resolved path escapes project"),
            ModuleResolveError::Io => f.write_str("I/O error"),
        }
    }
}

impl std::error::Error for ModuleResolveError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn disk_provider(root: &Path) -> crate::resolve::provider::DiskProvider {
        crate::resolve::provider::DiskProvider::new(root).0
    }

    fn setup_test_tree() -> (tempfile::TempDir, PathBuf, Config, Matcher, TsConfig) {
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

        // Create tsconfig at root
        let tsconfig = TsConfig {
            base_url: Some(".".into()),
            paths: vec![
                crate::resolve::tsconfig::PathsEntry {
                    pattern: "@users/*".into(),
                    target: "src/users/*".into(),
                },
                crate::resolve::tsconfig::PathsEntry {
                    pattern: "@shared/*".into(),
                    target: "src/shared/*".into(),
                },
            ],
        };
        (dir, root, config, matcher, tsconfig)
    }

    #[test]
    fn resolves_relative_imports() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/create-user.dto.ts"),
            "export class C {}",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            "import { C } from './dto/create-user.dto';",
        )
        .unwrap();

        let from = Path::new("src/users/users.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("./dto/create-user.dto", from, &env).unwrap();
        assert_eq!(
            resolved,
            Some(PathBuf::from("src/users/dto/create-user.dto.ts"))
        );
    }

    #[test]
    fn resolves_directory_imports_via_index_ts() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(root.join("src/users/dto/index.ts"), "export class C {}").unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            "import { C } from './dto';",
        )
        .unwrap();

        let from = Path::new("src/users/users.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("./dto", from, &env).unwrap();
        assert_eq!(resolved, Some(PathBuf::from("src/users/dto/index.ts")));
    }

    #[test]
    fn resolves_parent_imports() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        fs::create_dir_all(root.join("src/shared/dto")).unwrap();
        fs::create_dir_all(root.join("src/users")).unwrap();
        fs::write(
            root.join("src/shared/dto/pagination.dto.ts"),
            "export class P {}",
        )
        .unwrap();
        // From src/users/users.controller.ts, one .. goes to src/, then shared/dto/...
        fs::write(
            root.join("src/users/users.controller.ts"),
            "import { P } from '../shared/dto/pagination.dto';",
        )
        .unwrap();

        let from = Path::new("src/users/users.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("../shared/dto/pagination.dto", from, &env).unwrap();
        assert_eq!(
            resolved,
            Some(PathBuf::from("src/shared/dto/pagination.dto.ts"))
        );
    }

    #[test]
    fn resolves_tsconfig_paths_wildcard() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/create-user.dto.ts"),
            "export class C {}",
        )
        .unwrap();

        let from = Path::new("src/auth/auth.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("@users/dto/create-user.dto", from, &env).unwrap();
        assert_eq!(
            resolved,
            Some(PathBuf::from("src/users/dto/create-user.dto.ts"))
        );
    }

    #[test]
    fn rejects_escaping_paths() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        let from = Path::new("src/users/users.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("../../../etc/passwd", from, &env);
        assert!(matches!(resolved, Err(ModuleResolveError::EscapesProject)));
    }

    #[test]
    fn respects_excluded_dirs() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        fs::create_dir_all(root.join("src/dist/users")).unwrap();
        fs::write(root.join("src/dist/users/user.dto.ts"), "export class U {}").unwrap();

        let from = Path::new("src/users/users.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("../dist/users/user.dto", from, &env).unwrap();
        // Should be None because dist is excluded
        assert_eq!(resolved, None);
    }

    #[test]
    fn resolves_nodenext_js_extension_to_ts() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(
            root.join("src/users/dto/create-user.dto.ts"),
            "export class C {}",
        )
        .unwrap();

        let from = Path::new("src/users/users.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("./dto/create-user.dto.js", from, &env).unwrap();
        assert_eq!(
            resolved,
            Some(PathBuf::from("src/users/dto/create-user.dto.ts"))
        );
    }

    #[test]
    fn resolves_nodenext_js_extension_via_index_ts() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        fs::create_dir_all(root.join("src/users/dto")).unwrap();
        fs::write(root.join("src/users/dto/index.ts"), "export class C {}").unwrap();

        let from = Path::new("src/users/users.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("./dto/index.js", from, &env).unwrap();
        assert_eq!(resolved, Some(PathBuf::from("src/users/dto/index.ts")));
    }

    #[test]
    fn resolves_nodenext_mjs_and_cjs_extensions() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        fs::create_dir_all(root.join("src/users")).unwrap();
        fs::write(root.join("src/users/legacy.mts"), "export class M {}").unwrap();
        fs::write(root.join("src/users/commonjs.cts"), "export class C {}").unwrap();

        let from = Path::new("src/users/users.controller.ts");
        for (specifier, expected) in [
            ("./legacy.mjs", "src/users/legacy.mts"),
            ("./commonjs.cjs", "src/users/commonjs.cts"),
        ] {
            let env = ModuleEnv {
                source_prefix: Path::new("src"),
                config: &config,
                matcher: &matcher,
                tsconfig: Some(&tsconfig),
                provider: &disk_provider(&root),
                package_imports: None,
            };
            let resolved = resolve_module(specifier, from, &env).unwrap();
            assert_eq!(resolved, Some(PathBuf::from(expected)));
        }
    }

    #[test]
    fn js_extension_without_ts_source_is_none() {
        let (_dir, root, config, matcher, tsconfig) = setup_test_tree();
        let from = Path::new("src/users/users.controller.ts");
        let env = ModuleEnv {
            source_prefix: Path::new("src"),
            config: &config,
            matcher: &matcher,
            tsconfig: Some(&tsconfig),
            provider: &disk_provider(&root),
            package_imports: None,
        };
        let resolved = resolve_module("./missing.dto.js", from, &env).unwrap();
        assert_eq!(resolved, None);
    }
}
