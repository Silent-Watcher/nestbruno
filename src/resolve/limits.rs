//! Resource limits for the cross-file DTO resolver.
//!
//! These limits prevent pathological memory and CPU consumption. They are
//! internal implementation constants, not user configuration.
//!
//! # Boundary semantics
//!
//! All budgets share one rule: the operation that would exceed a budget is
//! rejected in full, and work already completed within budget stays valid.
//! For per-module budgets (`MAX_DEPENDENCY_FILES`, `MAX_TOTAL_DEPENDENCY_BYTES`,
//! `MAX_DTO_DECLARATIONS`, `MAX_SOURCE_BYTES`) this means a module is either
//! analyzed completely or not at all — a partially tracked module is never
//! published to the cache. Callers surface every rejection as
//! `DtoResolution::Unknown`, keeping the conservative "unknown, not empty"
//! contract. Counters use checked arithmetic; overflow saturates the counter
//! and the next budget check rejects.

/// Maximum re-export chain hops to follow (e.g., export { X } from './a' ->
/// export { X } from './b' -> class X ...). The prompt specifies 4.
pub const MAX_REEXPORT_HOPS: usize = 4;

/// Maximum number of dependency files resolved in a single analysis run
/// (incremental or full). This bounds memory and parsing work.
pub const MAX_DEPENDENCY_FILES: usize = 512;

/// Maximum number of import/export edges (specifier resolutions that name a
/// project module) established per analysis run.
pub const MAX_IMPORT_EDGES: usize = 4096;

/// Maximum aggregate source bytes of all resolved dependency files.
/// 4 MiB (similar scale to MAX_SOURCE_BYTES per file * file count).
pub const MAX_TOTAL_DEPENDENCY_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of DTO declarations extracted across all resolved files.
pub const MAX_DTO_DECLARATIONS: usize = 1024;

/// Maximum source bytes per file (re-exported from extract for cross-file
/// resolution to use the same limit).
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;

/// Maximum properties per DTO class/interface (re-exported from extract).
pub const MAX_DTO_PROPERTIES: usize = 64;
