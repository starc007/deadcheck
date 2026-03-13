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
use std::path::Path;

use petgraph::Direction;

use crate::confidence;
use crate::graph::DependencyGraph;
use crate::types::{AnalysisResult, DeadFile, ExportKind, FileId, UnusedExport};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run the full dead-code analysis over the dependency graph.
///
/// Returns an [`AnalysisResult`] containing dead files, unused exports, and
/// unused npm dependencies.
pub fn analyze(graph: &DependencyGraph, _root: &Path) -> AnalysisResult {
    let reachable = compute_reachable(graph);
    let total_files = graph.files.len();
    let reachable_count = reachable.len();

    let dead_files = collect_dead_files(graph, &reachable);
    let unused_exports = collect_unused_exports(graph, &reachable);

    AnalysisResult {
        dead_files,
        unused_exports,
        unused_dependencies: vec![], // Phase 2: requires package.json parsing
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
                .any(|e| *e.weight() == crate::graph::EdgeKind::Dynamic);

            let (confidence, signals) = confidence::score(
                file,
                graph,
                incoming_count,
                has_dynamic_import_incoming,
            );

            Some(DeadFile {
                path: file.relative_path.display().to_string(),
                confidence,
                signals,
            })
        })
        .collect();

    // Sort: highest confidence first, then alphabetically.
    dead.sort_unstable_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then(a.path.cmp(&b.path))
    });

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
fn is_symbol_imported(
    graph: &DependencyGraph,
    target_file_id: FileId,
    symbol_name: &str,
) -> bool {
    // Walk all incoming edges to find files that import from `target_file_id`.
    for importer_id in graph.graph.neighbors_directed(target_file_id, Direction::Incoming) {
        let Some(importer_file) = graph.files.get(importer_id.index()) else {
            continue;
        };

        for import in &importer_file.imports {
            // Resolve the import specifier to see if it points at our target.
            // (We do a quick file_map lookup instead of re-running the resolver.)
            let resolved = crate::resolver::resolve(
                &import.specifier,
                &importer_file.path,
                &[], // no aliases in Phase 1
            );

            let points_at_target = match resolved {
                crate::types::Resolution::File(p) => {
                    graph.file_map.get(&p).copied() == Some(target_file_id)
                }
                _ => false,
            };

            if !points_at_target {
                continue;
            }

            // The import points at our file. Check if it imports our symbol.
            for name in &import.imported_names {
                if name == "*" || name == symbol_name {
                    return true;
                }
            }
        }
    }

    false
}
