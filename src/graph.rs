//! Dependency graph construction and entry point detection.
//!
//! This module takes the flat list of [`FileInfo`] structs produced by the
//! parser and builds a directed graph where:
//!
//! - Each **node** is a source file.
//! - Each **edge** `A → B` means "file A imports file B".
//!
//! Entry points are detected heuristically from file names and paths. Users
//! can supply additional entry points via `--entry`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use petgraph::graph::DiGraph;

use crate::resolver;
use crate::types::{FileId, FileInfo, ImportKind, Resolution};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The fully-constructed dependency graph.
pub struct DependencyGraph {
    /// Directed graph. Each node is a [`FileId`]; each edge carries an
    /// [`EdgeKind`] indicating how the import was written.
    pub graph: DiGraph<FileId, EdgeKind>,

    /// Maps an absolute file path to its node index.
    pub file_map: HashMap<PathBuf, FileId>,

    /// All parsed file information, indexed by [`FileId`].
    pub files: Vec<FileInfo>,

    /// Entry points detected or supplied by the user.
    pub entry_points: Vec<FileId>,

    /// npm package names seen in any import across the project.
    ///
    /// Populated now; consumed in Phase 2 for unused dependency detection.
    #[allow(dead_code)]
    pub external_packages: Vec<String>,
}

/// How an import was written (stored on each graph edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// `import x from "..."` or `import { a } from "..."`
    Static,
    /// `import("...")`
    Dynamic,
    /// `export { x } from "..."` or `export * from "..."`
    ReExport,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a [`DependencyGraph`] from the parsed file information.
///
/// # Arguments
///
/// * `root` — absolute project root (used for relative path display and
///   entry point heuristics).
/// * `files` — all parsed [`FileInfo`] structs.
/// * `extra_entries` — paths supplied via `--entry`. These are added on top
///   of the auto-detected entry points.
pub fn build(root: &Path, files: Vec<FileInfo>, extra_entries: &[PathBuf]) -> Result<DependencyGraph> {
    let mut graph: DiGraph<FileId, EdgeKind> = DiGraph::new();
    let mut file_map: HashMap<PathBuf, FileId> = HashMap::new();

    // ------------------------------------------------------------------
    // Pass 1: Insert every file as a graph node.
    // ------------------------------------------------------------------
    // We store the NodeIndex as the node weight (self-referential) so callers
    // can retrieve the FileId directly from the graph without a separate lookup.
    let mut indexed_files: Vec<FileInfo> = Vec::with_capacity(files.len());

    for file in files {
        let node = graph.add_node(FileId::default()); // placeholder weight
        file_map.insert(file.path.clone(), node);
        indexed_files.push(file);
        // Fix up the node weight to equal its own index.
        *graph.node_weight_mut(node).unwrap() = node;
    }

    // ------------------------------------------------------------------
    // Pass 2: Add edges for each import relationship.
    // ------------------------------------------------------------------
    let mut external_packages: Vec<String> = Vec::new();

    // No tsconfig alias support in Phase 1 — pass an empty slice.
    let path_aliases: Vec<(String, Vec<PathBuf>)> = vec![];

    for file in &indexed_files {
        let Some(&importer_id) = file_map.get(&file.path) else {
            continue;
        };

        for import in &file.imports {
            let edge_kind = match import.kind {
                ImportKind::Static => EdgeKind::Static,
                ImportKind::Dynamic => EdgeKind::Dynamic,
                ImportKind::ReExport => EdgeKind::ReExport,
            };

            match resolver::resolve(&import.specifier, &file.path, &path_aliases) {
                Resolution::File(resolved_path) => {
                    if let Some(&importee_id) = file_map.get(&resolved_path) {
                        graph.add_edge(importer_id, importee_id, edge_kind);
                    }
                    // If the resolved path isn't in file_map it's outside the
                    // project root (e.g. a symlinked dependency) — skip it.
                }
                Resolution::External(pkg) => {
                    external_packages.push(pkg);
                }
                Resolution::Unresolvable(_) => {
                    // Not resolvable — no edge added.
                }
            }
        }
    }

    // Deduplicate external packages.
    external_packages.sort_unstable();
    external_packages.dedup();

    // ------------------------------------------------------------------
    // Detect entry points
    // ------------------------------------------------------------------
    let mut entry_points: Vec<FileId> = detect_entry_points(root, &indexed_files, &file_map);

    // Add any paths supplied via --entry.
    for extra in extra_entries {
        let canonical = extra
            .canonicalize()
            .unwrap_or_else(|_| extra.clone());
        if let Some(&id) = file_map.get(&canonical) {
            if !entry_points.contains(&id) {
                entry_points.push(id);
            }
        }
    }

    Ok(DependencyGraph {
        graph,
        file_map,
        files: indexed_files,
        entry_points,
        external_packages,
    })
}

// ---------------------------------------------------------------------------
// Entry point detection
// ---------------------------------------------------------------------------

/// Heuristically determine which files are entry points.
///
/// Detection runs in priority order. Files can be detected as entry points
/// by multiple rules; we deduplicate before returning.
fn detect_entry_points(
    root: &Path,
    files: &[FileInfo],
    file_map: &HashMap<PathBuf, FileId>,
) -> Vec<FileId> {
    let mut entries: Vec<FileId> = Vec::new();

    for file in files {
        if is_entry_point(root, &file.relative_path) {
            if let Some(&id) = file_map.get(&file.path) {
                if !entries.contains(&id) {
                    entries.push(id);
                }
            }
        }
    }

    entries
}

/// Returns `true` if `relative_path` looks like a project entry point.
fn is_entry_point(root: &Path, relative_path: &Path) -> bool {
    // Common entry-point file names (at any depth for framework routes,
    // at root / src/ depth for conventional entry points).
    let file_name = relative_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Exact matches for common entry file names at the top level or in `src/`.
    if matches_root_entry(relative_path, file_name) {
        return true;
    }

    // Next.js App Router conventions.
    if is_nextjs_app_route(relative_path, file_name) {
        return true;
    }

    // Next.js Pages Router: all files directly under `pages/`.
    if is_nextjs_page(relative_path) {
        return true;
    }

    // Remix routes.
    if is_remix_route(relative_path) {
        return true;
    }

    // Config / tooling files at project root.
    if is_root_config(root, relative_path, file_name) {
        return true;
    }

    false
}

/// Conventional entry file names like `index.ts`, `main.ts`, `app.tsx`.
fn matches_root_entry(relative_path: &Path, file_name: &str) -> bool {
    // Only apply filename heuristics to the root or one level deep (e.g. `src/`).
    let depth = relative_path.components().count();
    if depth > 2 {
        return false;
    }

    matches!(
        file_name,
        "index.ts"
            | "index.tsx"
            | "index.js"
            | "index.jsx"
            | "main.ts"
            | "main.tsx"
            | "main.js"
            | "app.ts"
            | "app.tsx"
            | "app.js"
            | "server.ts"
            | "server.js"
    )
}

/// Next.js App Router: `page.tsx`, `layout.tsx`, `route.ts`, etc. inside `app/`.
fn is_nextjs_app_route(relative_path: &Path, file_name: &str) -> bool {
    let in_app_dir = relative_path
        .components()
        .any(|c| c.as_os_str() == "app");

    if !in_app_dir {
        return false;
    }

    // Strip extension to compare stem.
    let stem = relative_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    matches!(
        stem,
        "page" | "layout" | "route" | "loading" | "error" | "not-found" | "middleware" | "template"
    ) || file_name == "middleware.ts"
        || file_name == "middleware.js"
}

/// Next.js Pages Router: all files directly under `pages/`, except internals.
fn is_nextjs_page(relative_path: &Path) -> bool {
    let components: Vec<_> = relative_path.components().collect();
    if components.len() != 2 {
        return false;
    }

    let dir = components[0].as_os_str().to_string_lossy();
    if dir != "pages" {
        return false;
    }

    // `_app.tsx` and `_document.tsx` are special but still entry points.
    true
}

/// Remix routes: files under `app/routes/`.
fn is_remix_route(relative_path: &Path) -> bool {
    let mut components = relative_path.components();
    let first = components
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    let second = components
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());

    matches!((first.as_deref(), second.as_deref()), (Some("app"), Some("routes")))
}

/// Config files at the project root (vite.config.ts, next.config.mjs, etc.).
fn is_root_config(root: &Path, relative_path: &Path, file_name: &str) -> bool {
    // Only look at files directly in the project root.
    if relative_path.parent() != Some(Path::new("")) && relative_path.parent() != Some(root) {
        if relative_path.components().count() != 1 {
            return false;
        }
    }

    matches!(
        file_name,
        "vite.config.ts"
            | "vite.config.js"
            | "next.config.js"
            | "next.config.mjs"
            | "next.config.ts"
            | "astro.config.mjs"
            | "astro.config.ts"
            | "svelte.config.js"
            | "nuxt.config.ts"
            | "remix.config.js"
    )
}
