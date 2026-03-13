//! Dead code analysis.
//!
//! Runs a multi-source BFS from all entry points to find every reachable file.
//! Files not visited during BFS are dead. Each dead file is then scored by
//! [`crate::confidence`].
//!
//! The module also detects:
//! - **Unused exports** — symbols exported by reachable files but never
//!   imported anywhere.
//! - **Unused npm dependencies** — packages listed in `package.json` that
//!   are never referenced in any import statement.

use std::collections::{HashSet, VecDeque};

use petgraph::Direction;

use crate::confidence;
use crate::config::ProjectConfig;
use crate::graph::DependencyGraph;
use crate::types::{AnalysisResult, DeadFile, ExportKind, FileId, UnusedExport};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run the full dead-code analysis over the dependency graph.
///
/// Returns an [`AnalysisResult`] containing dead files, unused exports, and
/// unused npm dependencies.
pub fn analyze(graph: &DependencyGraph, cfg: &ProjectConfig) -> AnalysisResult {
    let reachable = compute_reachable(graph);
    let total_files = graph.files.len();
    let reachable_count = reachable.len();

    let dead_files = collect_dead_files(graph, &reachable);
    let unused_exports = collect_unused_exports(graph, &reachable);
    let unused_dependencies = collect_unused_dependencies(graph, cfg);

    AnalysisResult {
        dead_files,
        unused_exports,
        unused_dependencies,
        reachable_count,
        total_files,
    }
}

// ---------------------------------------------------------------------------
// Reachability (multi-source BFS)
// ---------------------------------------------------------------------------

/// Compute the set of all files reachable from any entry point.
///
/// Uses a standard multi-source BFS: all entry points are enqueued before
/// traversal begins, so reachability from *any* entry point is captured in a
/// single pass.
///
/// Dynamic import edges are traversed (their targets are considered reachable)
/// but callers can inspect `EdgeKind` to apply lower confidence scores.
fn compute_reachable(graph: &DependencyGraph) -> HashSet<FileId> {
    let mut visited: HashSet<FileId> = HashSet::new();
    let mut queue: VecDeque<FileId> = VecDeque::new();

    // Seed the queue with all entry points simultaneously.
    for &entry in &graph.entry_points {
        queue.push_back(entry);
    }

    while let Some(node) = queue.pop_front() {
        // Skip already-visited nodes to handle cycles correctly.
        if !visited.insert(node) {
            continue;
        }

        // Enqueue all files that this file imports (outgoing edges).
        for neighbor in graph.graph.neighbors_directed(node, Direction::Outgoing) {
            queue.push_back(neighbor);
        }
    }

    visited
}

// ---------------------------------------------------------------------------
// Dead file collection
// ---------------------------------------------------------------------------

fn collect_dead_files(graph: &DependencyGraph, reachable: &HashSet<FileId>) -> Vec<DeadFile> {
    let mut dead: Vec<DeadFile> = graph
        .files
        .iter()
        .filter_map(|file| {
            let id = *graph.file_map.get(&file.path)?;

            if reachable.contains(&id) {
                return None; // Reachable — not dead.
            }

            let incoming_count = graph
                .graph
                .neighbors_directed(id, Direction::Incoming)
                .count();

            let has_dynamic_import_incoming = graph
                .graph
                .edges_directed(id, Direction::Incoming)
                .any(|e| e.weight().kind == crate::graph::EdgeKind::Dynamic);

            let (confidence, signals) =
                confidence::score(file, graph, incoming_count, has_dynamic_import_incoming);

            Some(DeadFile {
                path: file.relative_path.display().to_string(),
                confidence,
                signals,
            })
        })
        .collect();

    // Sort: highest confidence first, then alphabetically.
    dead.sort_unstable_by(|a, b| b.confidence.cmp(&a.confidence).then(a.path.cmp(&b.path)));

    dead
}

// ---------------------------------------------------------------------------
// Unused export detection
// ---------------------------------------------------------------------------

/// Find exports in reachable files that are never imported by any other file.
///
/// For each exported symbol in a reachable file, we check whether any other
/// file's import edges reference that symbol by name (or via `*`). If nothing
/// imports it, the export is considered unused.
fn collect_unused_exports(
    graph: &DependencyGraph,
    reachable: &HashSet<FileId>,
) -> Vec<UnusedExport> {
    let mut unused: Vec<UnusedExport> = Vec::new();

    for file in &graph.files {
        let Some(&file_id) = graph.file_map.get(&file.path) else {
            continue;
        };

        // Only check reachable files — dead files are already reported.
        if !reachable.contains(&file_id) {
            continue;
        }

        for export in &file.exports {
            // `export * from "..."` and type-only symbols are skipped.
            if export.kind == ExportKind::ReExportAll || export.name == "*" {
                continue;
            }

            // Next.js App Router exports consumed directly by the framework
            // (e.g. `generateMetadata`, `metadata`, `viewport`) are never
            // imported by user code — skip them to avoid false positives.
            let file_path_str = file.relative_path.to_string_lossy();
            if is_nextjs_framework_export(&file_path_str, &export.name) {
                continue;
            }

            let is_used = is_symbol_imported(graph, file_id, &export.name);

            if !is_used {
                unused.push(UnusedExport {
                    file_path: file.relative_path.display().to_string(),
                    symbol_name: export.name.clone(),
                    kind: export.kind,
                });
            }
        }
    }

    // Sort by file path, then symbol name for deterministic output.
    unused.sort_unstable_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.symbol_name.cmp(&b.symbol_name))
    });

    unused
}

/// Returns `true` if any file imports `symbol_name` from `target_file_id`.
///
/// We walk the incoming edges of `target_file_id` directly. Because
/// `imported_names` was recorded on the edge at graph-construction time
/// (when we already had the correct resolved path), this avoids re-resolving
/// specifiers here — which was error-prone due to path canonicalization
/// differences between the scanner and the resolver.
fn is_symbol_imported(graph: &DependencyGraph, target_file_id: FileId, symbol_name: &str) -> bool {
    for edge in graph
        .graph
        .edges_directed(target_file_id, Direction::Incoming)
    {
        let data = edge.weight();
        for name in &data.imported_names {
            if name == "*" || name == symbol_name {
                return true;
            }
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Unused npm dependency detection
// ---------------------------------------------------------------------------

/// Returns `true` if the symbol is a Next.js App Router export that is consumed
/// directly by the Next.js runtime rather than imported by user code.
///
/// These exports must never be flagged as "unused" even when no other file
/// imports them — Next.js reads them via its own internal file-system routing.
fn is_nextjs_framework_export(file_relative_path: &str, symbol_name: &str) -> bool {
    let path = file_relative_path.to_lowercase();

    // Only applies inside the App Router `app/` directory.
    if !path.starts_with("app/") && !path.contains("/app/") {
        return false;
    }

    // Full list of Next.js App Router reserved export names.
    // See: https://nextjs.org/docs/app/api-reference/file-conventions
    matches!(
        symbol_name,
        // Page / layout component
        "default"
            // Static and dynamic metadata
            | "metadata"
            | "generateMetadata"
            // Viewport (Next.js 14+)
            | "viewport"
            | "generateViewport"
            // Static params for dynamic routes
            | "generateStaticParams"
            // Route segment config
            | "dynamic"
            | "dynamicParams"
            | "revalidate"
            | "fetchCache"
            | "runtime"
            | "preferredRegion"
            | "maxDuration"
            // Image route handlers
            | "size"
            | "contentType"
            | "alt"
            // HTTP method handlers (route.ts)
            | "GET"
            | "POST"
            | "PUT"
            | "PATCH"
            | "DELETE"
            | "HEAD"
            | "OPTIONS"
    )
}

// ---------------------------------------------------------------------------
// Unused npm dependency detection
// ---------------------------------------------------------------------------

/// Find npm packages listed in `package.json` that are never imported.
///
/// Compares the full set of `dependencies` + `devDependencies` against the
/// set of external specifiers actually seen during parsing. Packages in
/// `cfg.ignore_dependencies` are always skipped.
fn collect_unused_dependencies(graph: &DependencyGraph, cfg: &ProjectConfig) -> Vec<String> {
    let checked = cfg.all_checked_dependencies();

    if checked.is_empty() {
        // No package.json or no dependencies — nothing to check.
        return vec![];
    }

    // Collect all external package names seen in any import across the project.
    let seen: HashSet<&str> = graph.external_packages.iter().map(String::as_str).collect();

    let mut unused: Vec<String> = checked
        .into_iter()
        .filter(|dep| !seen.contains(dep.as_str()))
        .collect();

    unused.sort_unstable();
    unused
}
