//! Import specifier resolver.
//!
//! Takes a raw specifier string (e.g. `"./utils"`, `"react"`, `"@/hooks"`)
//! and the absolute path of the file containing the import, and resolves it
//! to one of three outcomes:
//!
//! - [`Resolution::File`] — a project-local file was found on disk.
//! - [`Resolution::External`] — an npm package (not resolved to a file).
//! - [`Resolution::Unresolvable`] — the specifier could not be resolved
//!   (dynamic expression, missing file, or unsupported feature).

use std::path::{Path, PathBuf};

use crate::types::Resolution;

/// Extensions tried in order when resolving an extension-less specifier.
const EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mts", "mjs", "cts", "cjs"];

/// Index file names tried when resolving a directory specifier.
const INDEX_FILES: &[&str] = &[
    "index.ts",
    "index.tsx",
    "index.js",
    "index.jsx",
    "index.mts",
    "index.mjs",
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve `specifier` (as written in `importing_file`) to a [`Resolution`].
///
/// # Arguments
///
/// * `specifier` — the raw string from the import statement.
/// * `importing_file` — absolute path of the file containing the import.
/// * `path_aliases` — tsconfig `paths` mappings (prefix → list of roots).
///   Pass an empty slice when no aliases are configured.
pub fn resolve(
    specifier: &str,
    importing_file: &Path,
    path_aliases: &[(String, Vec<PathBuf>)],
) -> Resolution {
    // Relative imports — the most common case.
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let base = match importing_file.parent() {
            Some(p) => p,
            None => return Resolution::Unresolvable(specifier.to_string()),
        };
        return resolve_path(base.join(specifier));
    }

    // Absolute path (unusual but valid in some setups).
    if specifier.starts_with('/') {
        return resolve_path(PathBuf::from(specifier));
    }

    // Path alias (e.g. `@/components/Button` → `src/components/Button`).
    if let Some(expanded) = expand_alias(specifier, path_aliases) {
        return resolve_path(expanded);
    }

    // Everything else is treated as an npm package name.
    Resolution::External(npm_package_name(specifier))
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

/// Try to resolve `path` to an existing file, trying extensions and index
/// files if needed.
///
/// The returned path is **canonicalized** (all `..` and symlinks resolved) so
/// it matches the absolute keys stored in the dependency graph's `file_map`.
/// Without this, a specifier like `"../controllers/authController"` would
/// resolve to a path containing `..` that looks different from the scanner's
/// canonical entry for the same file, causing the file to appear unreachable.
fn resolve_path(path: PathBuf) -> Resolution {
    // 1. Exact path exists as a file.
    if path.is_file() {
        return Resolution::File(canonicalize_or_keep(path));
    }

    // 2. Extension-less specifier — try adding known extensions.
    if path.extension().is_none() || !is_js_ts_extension(&path) {
        for ext in EXTENSIONS {
            let candidate = path.with_extension(ext);
            if candidate.is_file() {
                return Resolution::File(canonicalize_or_keep(candidate));
            }
        }
    }

    // 3. Directory specifier — try index files inside it.
    if path.is_dir() {
        for index in INDEX_FILES {
            let candidate = path.join(index);
            if candidate.is_file() {
                return Resolution::File(canonicalize_or_keep(candidate));
            }
        }
    }

    Resolution::Unresolvable(path.display().to_string())
}

/// Canonicalize `path`, falling back to the original if the OS call fails.
///
/// Canonicalization resolves `..` components and symlinks so the returned path
/// matches the absolute paths produced by the directory scanner.
fn canonicalize_or_keep(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// Returns `true` if the path already has a JS/TS extension.
fn is_js_ts_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "ts" | "jsx" | "tsx" | "mjs" | "cjs" | "mts" | "cts")
    )
}

// ---------------------------------------------------------------------------
// Path alias expansion
// ---------------------------------------------------------------------------

/// Attempt to expand `specifier` using the provided alias table.
///
/// Each entry in `aliases` is a `(prefix, roots)` pair where `prefix` may
/// end with `*` (wildcard). If `specifier` matches the prefix, the wildcard
/// portion is substituted into each root and the first existing path is
/// returned.
fn expand_alias(specifier: &str, aliases: &[(String, Vec<PathBuf>)]) -> Option<PathBuf> {
    for (prefix, roots) in aliases {
        if let Some(rest) = match_alias_prefix(specifier, prefix) {
            for root in roots {
                // Replace the trailing `*` in the root pattern with `rest`.
                let root_str = root.to_string_lossy();
                let expanded = if root_str.ends_with('*') {
                    PathBuf::from(format!(
                        "{}{}",
                        root_str.strip_suffix('*').unwrap_or(&root_str),
                        rest
                    ))
                } else if rest.is_empty() {
                    root.clone()
                } else {
                    root.join(rest)
                };

                // Return the first root that actually leads to a resolvable file.
                if let Resolution::File(path) = resolve_path(expanded) {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Try to match `specifier` against `prefix`.
///
/// Returns `Some(rest)` where `rest` is the part of `specifier` after the
/// matched prefix. The prefix may end with `*` (matches any suffix) or be
/// an exact string.
fn match_alias_prefix<'a>(specifier: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.ends_with('*') {
        let base = prefix.strip_suffix('*').unwrap_or(prefix);
        specifier.strip_prefix(base)
    } else if specifier == prefix {
        Some("")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// npm package name normalisation
// ---------------------------------------------------------------------------

/// Strip sub-path specifiers from an npm package name.
///
/// `"@radix-ui/react-dialog/dist/index"` → `"@radix-ui/react-dialog"`
/// `"lodash/fp"` → `"lodash"`
fn npm_package_name(specifier: &str) -> String {
    if specifier.starts_with('@') {
        // Scoped package: keep the first two path segments.
        let mut parts = specifier.splitn(3, '/');
        let scope = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("");
        if name.is_empty() {
            specifier.to_string()
        } else {
            format!("{scope}/{name}")
        }
    } else {
        // Unscoped package: keep only the first segment.
        specifier.split('/').next().unwrap_or(specifier).to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_unscoped_subpath() {
        assert_eq!(npm_package_name("lodash/fp"), "lodash");
    }

    #[test]
    fn strips_scoped_subpath() {
        assert_eq!(
            npm_package_name("@radix-ui/react-dialog/dist"),
            "@radix-ui/react-dialog"
        );
    }

    #[test]
    fn keeps_bare_scoped_package() {
        assert_eq!(
            npm_package_name("@radix-ui/react-dialog"),
            "@radix-ui/react-dialog"
        );
    }

    #[test]
    fn alias_exact_match() {
        assert_eq!(match_alias_prefix("@/utils", "@/*"), Some("utils"));
    }

    #[test]
    fn alias_no_match() {
        assert_eq!(match_alias_prefix("react", "@/*"), None);
    }
}
