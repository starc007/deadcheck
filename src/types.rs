//! Core data types shared across the entire pipeline.
//!
//! The pipeline flows through these types:
//! `FileInfo` (parser) → `DependencyGraph` (graph) → `AnalysisResult` (analyzer)

use std::path::PathBuf;

use petgraph::graph::NodeIndex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// File identity
// ---------------------------------------------------------------------------

/// A lightweight, stable identifier for a file within the dependency graph.
///
/// Under the hood this is a petgraph `NodeIndex`. It is valid only for the
/// lifetime of a single `DependencyGraph` instance — do not persist it.
pub type FileId = NodeIndex;

// ---------------------------------------------------------------------------
// Parsed file data
// ---------------------------------------------------------------------------

/// All import/export information extracted from a single source file.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Absolute canonical path to the file.
    pub path: PathBuf,

    /// Path relative to the project root (used for display).
    pub relative_path: PathBuf,

    /// All import edges found in this file (static and re-exports).
    pub imports: Vec<ImportEdge>,

    /// All named/default exports declared in this file.
    pub exports: Vec<ExportedSymbol>,

    /// Dynamic `import()` specifiers found in this file.
    ///
    /// These may be template literals or variables and therefore unresolvable
    /// at analysis time. They are stored separately to apply confidence
    /// penalties to nearby files. Used in Phase 4.
    #[allow(dead_code)]
    pub dynamic_imports: Vec<String>,

    /// TypeScript type names referenced within this file's own body.
    ///
    /// Populated from `TsTypeRef` and `TsExprWithTypeArgs` nodes. Used to
    /// suppress false-positive "unused export" reports for symbols that are
    /// consumed internally (e.g. `mongoose.model<IMessage>(...)`).
    pub internal_type_refs: Vec<String>,
}

/// A single import relationship from one file to a specifier.
#[derive(Debug, Clone)]
pub struct ImportEdge {
    /// The raw import specifier as written in source (`"./utils"`, `"react"`, `"@/hooks"`).
    pub specifier: String,

    /// Whether this is a static import, dynamic import, or re-export.
    pub kind: ImportKind,

    /// The specific names imported from the specifier.
    ///
    /// - `"default"` represents a default import (`import Foo from "..."`)
    /// - `"*"` represents a namespace import (`import * as Foo from "..."`)
    /// - Any other string is a named import (`import { foo } from "..."`)
    pub imported_names: Vec<String>,
}

/// How an import was written in source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    /// `import x from "..."` or `import { a } from "..."`
    Static,
    /// `import("...")`  — may not be statically resolvable. Used in Phase 4.
    #[allow(dead_code)]
    Dynamic,
    /// `export { x } from "..."` or `export * from "..."`
    ReExport,
}

/// A symbol that a file makes available to other modules.
#[derive(Debug, Clone)]
pub struct ExportedSymbol {
    /// The exported name (`"default"`, `"foo"`, `"MyComponent"`, `"*"`).
    pub name: String,

    /// Whether this is a named, default, or star re-export.
    pub kind: ExportKind,
}

/// How a symbol is exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportKind {
    Named,
    Default,
    /// `export * from "..."`
    ReExportAll,
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// The outcome of resolving an import specifier to a filesystem path.
#[derive(Debug, Clone)]
#[allow(dead_code)] // inner strings used for error reporting in later phases
pub enum Resolution {
    /// Resolved to an absolute path of a project-local file.
    File(PathBuf),

    /// An npm package name — tracked for unused-dependency analysis but not
    /// added as a graph edge.
    External(String),

    /// Could not be resolved (dynamic specifier, missing file, unsupported
    /// feature). A warning is emitted but analysis continues.
    Unresolvable(String),
}

// ---------------------------------------------------------------------------
// Confidence
// ---------------------------------------------------------------------------

/// How confident the tool is that a file or symbol is actually dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Confidence::High => write!(f, "HIGH"),
            Confidence::Medium => write!(f, "MEDIUM"),
            Confidence::Low => write!(f, "LOW"),
        }
    }
}

/// A single signal that raised or lowered the confidence score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceSignal {
    pub kind: SignalKind,
    /// Positive values increase confidence that the file is dead.
    /// Negative values decrease it.
    pub delta: i32,
}

/// The individual signals used to compute a confidence score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalKind {
    /// No other file imports this file at all. (+40)
    NotImportedByAnyFile,
    /// The file is not an entry point. (+20)
    NotAnEntryPoint,
    /// The file path does not match any known framework route pattern. (+15)
    NoMatchingFrameworkPattern,
    /// At least one dynamic `import()` in the codebase may reference this file. (-35)
    HasDynamicImportReferring,
    /// The file only re-exports from other modules (barrel file). (-20)
    IsBarrelFile,
    /// The file path matches a known framework route pattern. (-30)
    MatchesRoutePattern,
    /// The file appears to be a test file. (-10)
    IsTestFile,
    /// The file uses `export *` which may expose it indirectly. (-10)
    HasExportStar,
    /// The file lives in a `public/` or `assets/` directory. (-25)
    InPublicDirectory,
}

impl SignalKind {
    /// The score delta this signal contributes.
    pub fn delta(self) -> i32 {
        match self {
            Self::NotImportedByAnyFile => 40,
            Self::NotAnEntryPoint => 20,
            Self::NoMatchingFrameworkPattern => 15,
            Self::HasDynamicImportReferring => -35,
            Self::IsBarrelFile => -20,
            Self::MatchesRoutePattern => -30,
            Self::IsTestFile => -10,
            Self::HasExportStar => -10,
            Self::InPublicDirectory => -25,
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis output
// ---------------------------------------------------------------------------

/// The full result of a dead-code analysis run.
#[derive(Debug, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// Files that are not reachable from any entry point.
    pub dead_files: Vec<DeadFile>,

    /// Exports that are declared but never imported by any other file.
    pub unused_exports: Vec<UnusedExport>,

    /// npm packages listed in `package.json` that are never imported.
    pub unused_dependencies: Vec<String>,

    /// Number of files that ARE reachable from entry points.
    pub reachable_count: usize,

    /// Total number of JS/TS files found in the project.
    pub total_files: usize,
}

/// A file determined to be unreachable from all entry points.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeadFile {
    /// Path relative to the project root.
    pub path: String,

    /// Confidence level for this result.
    pub confidence: Confidence,

    /// The individual signals that produced this confidence score.
    pub signals: Vec<ConfidenceSignal>,
}

/// An export that is declared but never consumed.
#[derive(Debug, Serialize, Deserialize)]
pub struct UnusedExport {
    /// Path of the file containing the unused export.
    pub file_path: String,

    /// The exported symbol name.
    pub symbol_name: String,

    pub kind: ExportKind,
}
