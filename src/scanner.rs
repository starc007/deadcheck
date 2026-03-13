//! Filesystem scanner.
//!
//! Walks the project directory and returns all JS/TS source files, respecting
//! `.gitignore` rules and a hardcoded list of directories that should always
//! be excluded (e.g. `node_modules`, `dist`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;

/// Directories that are always excluded regardless of `.gitignore`.
const ALWAYS_IGNORE: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    ".next",
    "out",
    ".turbo",
    ".cache",
    "coverage",
    ".git",
];

/// File extensions treated as JavaScript or TypeScript source files.
const JS_TS_EXTENSIONS: &[&str] = &["js", "ts", "jsx", "tsx", "mjs", "cjs", "mts", "cts"];

/// Walk `root` and return every JS/TS file that should be analysed.
///
/// # Arguments
///
/// * `root` — absolute path to the project root.
/// * `extra_ignore` — additional glob patterns supplied via `--ignore`.
///
/// # Errors
///
/// Returns an error if the `ignore` crate cannot build the walker (e.g. the
/// root directory does not exist or cannot be read).
pub fn scan(root: &Path, extra_ignore: &[String]) -> Result<Vec<PathBuf>> {
    let mut builder = WalkBuilder::new(root);

    // Respect .gitignore files automatically (enabled by default in `ignore`).
    builder.git_ignore(true);
    builder.git_global(true);
    builder.git_exclude(true);

    // Build a GlobSet from the caller-supplied patterns so we can check
    // them inside the single `filter_entry` closure.
    let mut glob_builder = GlobSetBuilder::new();
    for pattern in extra_ignore {
        let glob =
            Glob::new(pattern).with_context(|| format!("Invalid ignore pattern: {pattern}"))?;
        glob_builder.add(glob);
    }
    let extra_globs = glob_builder
        .build()
        .context("Failed to build ignore pattern set")?;

    // A single `filter_entry` combines the hardcoded directory exclusions
    // with any caller-supplied glob patterns.
    builder.filter_entry(move |entry| {
        let name = entry.file_name().to_string_lossy();

        if ALWAYS_IGNORE.contains(&name.as_ref()) {
            return false;
        }

        // Check caller-supplied glob patterns against the full path.
        if !extra_globs.is_empty() && extra_globs.is_match(entry.path()) {
            return false;
        }

        true
    });

    // Collect all matching files in parallel using `ignore`'s built-in
    // parallel walker. We convert back to a plain Vec<PathBuf> for the
    // next pipeline stage.
    let mut files: Vec<PathBuf> = builder
        .build()
        .filter_map(|entry| {
            let entry = entry.ok()?;

            // Only files (not directories or symlinks to directories).
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                return None;
            }

            // Only JS/TS source files. Skip `.d.ts` declaration files —
            // they are consumed by the TypeScript compiler, not the bundler.
            let path = entry.into_path();
            if is_js_ts_source(&path) {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    // Sort for deterministic output across runs.
    files.sort_unstable();

    Ok(files)
}

/// Returns `true` if the path is a JS/TS source file that should be analysed.
///
/// Declaration files (`.d.ts`) are explicitly excluded because they contain
/// no runtime code and are never imported by bundlers.
fn is_js_ts_source(path: &Path) -> bool {
    let Some(ext) = path.extension() else {
        return false;
    };

    let ext = ext.to_string_lossy();

    if !JS_TS_EXTENSIONS.contains(&ext.as_ref()) {
        return false;
    }

    // Exclude `.d.ts` and `.d.mts` declaration files.
    if let Some(stem) = path.file_stem() {
        let stem = stem.to_string_lossy();
        if stem.ends_with(".d") {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_js_ts_extensions() {
        for ext in &["js", "ts", "jsx", "tsx", "mjs", "mts"] {
            let path = PathBuf::from(format!("src/file.{ext}"));
            assert!(is_js_ts_source(&path), "should accept .{ext}");
        }
    }

    #[test]
    fn rejects_declaration_files() {
        let path = PathBuf::from("src/types.d.ts");
        assert!(!is_js_ts_source(&path), "should reject .d.ts files");
    }

    #[test]
    fn rejects_non_source_files() {
        for name in &["styles.css", "image.png", "README.md", "package.json"] {
            let path = PathBuf::from(name);
            assert!(!is_js_ts_source(&path), "should reject {name}");
        }
    }
}
