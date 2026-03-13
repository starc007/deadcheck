//! JS/TS source file parser.
//!
//! Uses [`swc_ecma_parser`] to parse each file into an AST, then walks the
//! AST with a [`Visit`] implementation that extracts all import and export
//! relationships.
//!
//! Parsing is embarrassingly parallel: each file is independent. We use
//! [`rayon`] to parse all files concurrently and collect results into a
//! [`DashMap`] before returning a sorted `Vec<FileInfo>`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dashmap::DashMap;
use indicatif::ProgressBar;
use rayon::prelude::*;
use swc_common::{sync::Lrc, FileName, SourceMap, GLOBALS};
use swc_ecma_ast::*;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_ecma_visit::{Visit, VisitWith};

use crate::types::{ExportKind, ExportedSymbol, FileInfo, ImportEdge, ImportKind};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse all `files` in parallel and return their [`FileInfo`] structs.
///
/// Files that fail to parse emit a warning to stderr and are skipped rather
/// than aborting the whole run, so a single broken file does not block the
/// analysis.
///
/// The returned `Vec` is sorted by path for deterministic output.
pub fn parse_all(root: &Path, files: &[PathBuf], progress: &ProgressBar) -> Result<Vec<FileInfo>> {
    // DashMap allows concurrent writes from rayon threads without a global lock.
    let results: DashMap<PathBuf, FileInfo> = DashMap::new();

    files.par_iter().for_each(|path| {
        match parse_file(root, path) {
            Ok(info) => {
                results.insert(path.clone(), info);
            }
            Err(err) => {
                // Non-fatal: warn and continue so one bad file doesn't stop
                // the whole analysis.
                eprintln!("Warning: skipping {} — {err}", path.display());
            }
        }
        progress.inc(1);
    });

    // Collect into a Vec sorted by path for deterministic ordering.
    let mut infos: Vec<FileInfo> = results.into_iter().map(|(_, v)| v).collect();
    infos.sort_unstable_by(|a, b| a.path.cmp(&b.path));

    Ok(infos)
}

// ---------------------------------------------------------------------------
// Single-file parsing
// ---------------------------------------------------------------------------

/// Parse a single file and extract its import/export information.
fn parse_file(root: &Path, path: &Path) -> Result<FileInfo> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read file: {}", path.display()))?;

    // Each thread creates its own SourceMap — SWC's SourceMap is not Send.
    let source_map = Lrc::new(SourceMap::default());
    let source_file =
        source_map.new_source_file(Lrc::new(FileName::Real(path.to_path_buf())), source.clone());

    // Use TypeScript syntax with JSX enabled — this handles .ts, .tsx, .js, .jsx.
    let syntax = Syntax::Typescript(TsSyntax {
        tsx: true,
        decorators: true,
        ..Default::default()
    });

    let lexer = Lexer::new(
        syntax,
        EsVersion::EsNext,
        StringInput::from(&*source_file),
        None,
    );
    let mut swc_parser = Parser::new_from(lexer);

    // `parse_module` returns a `Result` with SWC's own error type. Convert it
    // to an `anyhow::Error` so callers get a uniform error type.
    let module = swc_parser
        .parse_module()
        .map_err(|e| anyhow::anyhow!("Parse error in {}: {:?}", path.display(), e))?;

    // Walk the AST and collect imports/exports.
    let mut visitor = ImportExportVisitor::default();
    GLOBALS.set(&Default::default(), || {
        module.visit_with(&mut visitor);
    });

    let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    Ok(FileInfo {
        path: path.to_path_buf(),
        relative_path,
        imports: visitor.imports,
        exports: visitor.exports,
        dynamic_imports: visitor.dynamic_imports,
    })
}

// ---------------------------------------------------------------------------
// AST visitor
// ---------------------------------------------------------------------------

/// Walks a parsed module and collects all import and export information.
#[derive(Default)]
struct ImportExportVisitor {
    imports: Vec<ImportEdge>,
    exports: Vec<ExportedSymbol>,
    dynamic_imports: Vec<String>,
}

impl Visit for ImportExportVisitor {
    // ------------------------------------------------------------------
    // Static imports: `import foo from "..."`, `import { a, b } from "..."`
    // ------------------------------------------------------------------
    fn visit_import_decl(&mut self, node: &ImportDecl) {
        let specifier = node.src.value.to_string_lossy().into_owned();
        let mut imported_names: Vec<String> = Vec::new();

        for specifier in &node.specifiers {
            match specifier {
                ImportSpecifier::Default(_) => imported_names.push("default".to_string()),
                ImportSpecifier::Namespace(_) => imported_names.push("*".to_string()),
                ImportSpecifier::Named(named) => {
                    // Use the original name (before `as` renaming).
                    let name = match &named.imported {
                        Some(ModuleExportName::Ident(id)) => id.sym.to_string(),
                        Some(ModuleExportName::Str(s)) => s.value.to_string_lossy().into_owned(),
                        None => named.local.sym.to_string(),
                    };
                    imported_names.push(name);
                }
            }
        }

        self.imports.push(ImportEdge {
            specifier,
            kind: ImportKind::Static,
            imported_names,
        });
    }

    // ------------------------------------------------------------------
    // Named re-exports: `export { x } from "..."`, `export * from "..."`
    // ------------------------------------------------------------------
    fn visit_named_export(&mut self, node: &NamedExport) {
        if let Some(src) = &node.src {
            // This is `export { x } from "..."` — an import-and-re-export.
            let specifier = src.value.to_string_lossy().into_owned();
            let imported_names: Vec<String> = node
                .specifiers
                .iter()
                .map(|s| match s {
                    ExportSpecifier::Default(_) => "default".to_string(),
                    ExportSpecifier::Namespace(_) => "*".to_string(),
                    ExportSpecifier::Named(named) => match &named.orig {
                        ModuleExportName::Ident(id) => id.sym.to_string(),
                        ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
                    },
                })
                .collect();

            self.imports.push(ImportEdge {
                specifier,
                kind: ImportKind::ReExport,
                imported_names,
            });
        } else {
            // `export { x }` — re-exports a local binding, not a file.
            for spec in &node.specifiers {
                if let ExportSpecifier::Named(named) = spec {
                    let name = match &named.orig {
                        ModuleExportName::Ident(id) => id.sym.to_string(),
                        ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
                    };
                    self.exports.push(ExportedSymbol {
                        name,
                        kind: ExportKind::Named,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Star re-exports: `export * from "..."`
    // ------------------------------------------------------------------
    fn visit_export_all(&mut self, node: &ExportAll) {
        let specifier = node.src.value.to_string_lossy().into_owned();
        self.imports.push(ImportEdge {
            specifier,
            kind: ImportKind::ReExport,
            imported_names: vec!["*".to_string()],
        });
        // The file itself also re-exports everything.
        self.exports.push(ExportedSymbol {
            name: "*".to_string(),
            kind: ExportKind::ReExportAll,
        });
    }

    // ------------------------------------------------------------------
    // Default export: `export default function Foo() {}`, `export default 42`
    // ------------------------------------------------------------------
    fn visit_export_default_decl(&mut self, _node: &ExportDefaultDecl) {
        self.exports.push(ExportedSymbol {
            name: "default".to_string(),
            kind: ExportKind::Default,
        });
    }

    fn visit_export_default_expr(&mut self, _node: &ExportDefaultExpr) {
        self.exports.push(ExportedSymbol {
            name: "default".to_string(),
            kind: ExportKind::Default,
        });
    }

    // ------------------------------------------------------------------
    // Named declarations: `export function foo() {}`, `export const x = 1`
    // ------------------------------------------------------------------
    fn visit_export_decl(&mut self, node: &ExportDecl) {
        let names = exported_names_from_decl(&node.decl);
        for name in names {
            self.exports.push(ExportedSymbol {
                name,
                kind: ExportKind::Named,
            });
        }
    }

    // ------------------------------------------------------------------
    // Dynamic imports: `import("./foo")` or `import(someVar)`
    // ------------------------------------------------------------------
    fn visit_call_expr(&mut self, node: &CallExpr) {
        if let Callee::Import(_) = &node.callee {
            if let Some(first_arg) = node.args.first() {
                // Only record statically-known string specifiers.
                if let Expr::Lit(Lit::Str(s)) = first_arg.expr.as_ref() {
                    self.dynamic_imports
                        .push(s.value.to_string_lossy().into_owned());
                } else {
                    // Template literal or variable — not resolvable statically.
                    self.dynamic_imports.push("<dynamic>".to_string());
                }
            }
        }

        // Continue visiting children (the call may be nested).
        node.visit_children_with(self);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the exported identifier names from an `export` declaration.
fn exported_names_from_decl(decl: &Decl) -> Vec<String> {
    match decl {
        Decl::Fn(f) => vec![f.ident.sym.to_string()],
        Decl::Class(c) => vec![c.ident.sym.to_string()],
        Decl::Var(v) => v.decls.iter().flat_map(|d| pat_to_names(&d.name)).collect(),
        Decl::TsInterface(i) => vec![i.id.sym.to_string()],
        Decl::TsTypeAlias(t) => vec![t.id.sym.to_string()],
        Decl::TsEnum(e) => vec![e.id.sym.to_string()],
        Decl::TsModule(m) => match &m.id {
            TsModuleName::Ident(id) => vec![id.sym.to_string()],
            TsModuleName::Str(s) => vec![s.value.to_string_lossy().into_owned()],
        },
        Decl::Using(u) => u.decls.iter().flat_map(|d| pat_to_names(&d.name)).collect(),
    }
}

/// Recursively extract identifiers from a destructuring pattern.
fn pat_to_names(pat: &Pat) -> Vec<String> {
    match pat {
        Pat::Ident(id) => vec![id.id.sym.to_string()],
        Pat::Array(arr) => arr.elems.iter().flatten().flat_map(pat_to_names).collect(),
        Pat::Object(obj) => obj
            .props
            .iter()
            .flat_map(|p| match p {
                ObjectPatProp::KeyValue(kv) => pat_to_names(&kv.value),
                ObjectPatProp::Assign(a) => vec![a.key.id.sym.to_string()],
                ObjectPatProp::Rest(r) => pat_to_names(&r.arg),
            })
            .collect(),
        Pat::Rest(r) => pat_to_names(&r.arg),
        Pat::Assign(a) => pat_to_names(&a.left),
        Pat::Invalid(_) | Pat::Expr(_) => vec![],
    }
}
