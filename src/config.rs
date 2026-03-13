//! Project configuration loader.
//!
//! Reads three sources (in priority order, later overrides earlier):
//!
//! 1. **`package.json`** — npm dependency list + framework detection.
//! 2. **`tsconfig.json`** — `compilerOptions.paths` for import alias resolution.
//! 3. **`deadcheck.config.json`** — explicit overrides (entry points, ignore
//!    patterns, minimum confidence level).
//!
//! The resulting [`ProjectConfig`] is passed through the pipeline so every
//! module can read the same consistent configuration.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// All configuration needed to run the analysis pipeline.
#[derive(Debug, Default)]
pub struct ProjectConfig {
    /// Absolute path to the project root.
    pub root: PathBuf,

    /// Extra entry points from config / CLI (added on top of auto-detected ones).
    pub extra_entry_points: Vec<PathBuf>,

    /// Glob patterns to exclude from the scan.
    pub ignore_patterns: Vec<String>,

    /// Path alias mappings from `tsconfig.json` `compilerOptions.paths`.
    ///
    /// Format: `(alias_prefix, list_of_replacement_roots)`.
    /// Example: `("@/*", ["/abs/path/to/src/*"])`.
    pub path_aliases: Vec<(String, Vec<PathBuf>)>,

    /// Detected JavaScript/TypeScript framework.
    pub framework: Framework,

    /// All `dependencies` from `package.json`.
    pub dependencies: HashSet<String>,

    /// All `devDependencies` from `package.json`.
    pub dev_dependencies: HashSet<String>,

    /// npm package names to never flag as unused (e.g. tooling-only packages).
    pub ignore_dependencies: HashSet<String>,
}

impl ProjectConfig {
    /// Returns every npm package that should be checked for usage.
    ///
    /// `devDependencies` are included because many projects import dev packages
    /// in test files (e.g. `jest`, `@testing-library/react`). Users can opt out
    /// by adding them to `ignoreDependencies` in `deadcheck.config.json`.
    pub fn all_checked_dependencies(&self) -> HashSet<String> {
        self.dependencies
            .iter()
            .chain(self.dev_dependencies.iter())
            .filter(|dep| !self.ignore_dependencies.contains(*dep))
            .cloned()
            .collect()
    }
}

/// Detected JavaScript framework (used for entry-point heuristics).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Framework {
    #[default]
    Unknown,
    NextJs,
    Vite,
    CreateReactApp,
    Remix,
    SvelteKit,
    Astro,
    Nuxt,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load and merge all configuration sources.
///
/// Errors from missing or malformed config files are handled gracefully:
/// - Missing files → silently ignored (sensible defaults are used).
/// - JSON parse errors → warning to stderr, defaults used for that source.
pub fn load(
    root: &Path,
    cli_entries: &[PathBuf],
    cli_ignores: &[String],
    config_file: Option<&Path>,
) -> Result<ProjectConfig> {
    let mut cfg = ProjectConfig {
        root: root.to_path_buf(),
        ..Default::default()
    };

    // Always-ignored npm packages (tooling-only, never imported in source).
    cfg.ignore_dependencies = default_ignored_deps();

    // 1. Read package.json.
    load_package_json(root, &mut cfg);

    // 2. Read tsconfig.json.
    load_tsconfig(root, &mut cfg);

    // 3. Read deadcheck.config.json (or the path given via --config).
    let config_path = config_file
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root.join("deadcheck.config.json"));

    load_deadcheck_config(&config_path, &mut cfg);

    // 4. Apply CLI overrides (highest priority).
    for entry in cli_entries {
        let abs = if entry.is_absolute() {
            entry.clone()
        } else {
            root.join(entry)
        };
        cfg.extra_entry_points.push(abs);
    }
    cfg.ignore_patterns.extend_from_slice(cli_ignores);

    Ok(cfg)
}

// ---------------------------------------------------------------------------
// package.json
// ---------------------------------------------------------------------------

/// Raw shape of the fields we read from `package.json`.
#[derive(Debug, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PackageJson {
    main: Option<String>,
    module: Option<String>,
    dependencies: serde_json::Map<String, serde_json::Value>,
    dev_dependencies: serde_json::Map<String, serde_json::Value>,
}

fn load_package_json(root: &Path, cfg: &mut ProjectConfig) {
    let path = root.join("package.json");

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return, // No package.json — not a problem.
    };

    let pkg: PackageJson = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Warning: failed to parse package.json — {e}");
            return;
        }
    };

    // Collect dependency names.
    cfg.dependencies = pkg.dependencies.keys().cloned().collect();
    cfg.dev_dependencies = pkg.dev_dependencies.keys().cloned().collect();

    // Register package.json `main` / `module` fields as extra entry points.
    for field in [pkg.main, pkg.module].into_iter().flatten() {
        let abs = root.join(&field);
        if abs.exists() {
            cfg.extra_entry_points.push(abs);
        }
    }

    // Detect framework from dependencies.
    cfg.framework = detect_framework(&cfg.dependencies, &cfg.dev_dependencies);
}

/// Infer the framework from the dependency names.
fn detect_framework(deps: &HashSet<String>, dev: &HashSet<String>) -> Framework {
    let all: HashSet<&str> = deps
        .iter()
        .chain(dev.iter())
        .map(String::as_str)
        .collect();

    if all.contains("next") {
        Framework::NextJs
    } else if all.contains("@remix-run/react") || all.contains("@remix-run/node") {
        Framework::Remix
    } else if all.contains("@sveltejs/kit") {
        Framework::SvelteKit
    } else if all.contains("astro") {
        Framework::Astro
    } else if all.contains("nuxt") {
        Framework::Nuxt
    } else if all.contains("vite") {
        Framework::Vite
    } else if all.contains("react-scripts") {
        Framework::CreateReactApp
    } else {
        Framework::Unknown
    }
}

// ---------------------------------------------------------------------------
// tsconfig.json
// ---------------------------------------------------------------------------

/// Raw shape of the tsconfig fields we care about.
#[derive(Debug, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct TsConfig {
    compiler_options: TsCompilerOptions,
    extends: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct TsCompilerOptions {
    base_url: Option<String>,
    paths: serde_json::Map<String, serde_json::Value>,
}

fn load_tsconfig(root: &Path, cfg: &mut ProjectConfig) {
    // Check both tsconfig.json and tsconfig.app.json (Vite convention).
    let candidates = ["tsconfig.json", "tsconfig.app.json", "tsconfig.base.json"];

    for name in &candidates {
        let path = root.join(name);
        if path.exists() {
            load_tsconfig_file(&path, root, cfg);
            break;
        }
    }
}

fn load_tsconfig_file(path: &Path, root: &Path, cfg: &mut ProjectConfig) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };

    // Strip comments from JSON (tsconfig allows them — standard JSON doesn't).
    let stripped = strip_json_comments(&text);

    let ts: TsConfig = match serde_json::from_str(&stripped) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Warning: failed to parse {} — {e}", path.display());
            return;
        }
    };

    // If the tsconfig extends another file, load the parent first (best-effort).
    if let Some(extends) = &ts.extends {
        let parent = path
            .parent()
            .unwrap_or(root)
            .join(extends)
            .with_extension("json");
        if parent.exists() {
            load_tsconfig_file(&parent, root, cfg);
        }
    }

    // Resolve `baseUrl` to an absolute path.
    let base_dir = path.parent().unwrap_or(root);
    let base_url: PathBuf = match &ts.compiler_options.base_url {
        Some(u) => base_dir.join(u),
        None => base_dir.to_path_buf(),
    };

    // Parse `compilerOptions.paths` into alias mappings.
    for (alias, targets) in &ts.compiler_options.paths {
        let roots: Vec<PathBuf> = targets
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .map(|t| base_url.join(t))
            .collect();

        if !roots.is_empty() {
            cfg.path_aliases.push((alias.clone(), roots));
        }
    }
}

// ---------------------------------------------------------------------------
// deadcheck.config.json
// ---------------------------------------------------------------------------

/// Shape of the optional `deadcheck.config.json` file.
#[derive(Debug, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct DeadcheckConfig {
    entry_points: Vec<String>,
    ignore: Vec<String>,
    ignore_dependencies: Vec<String>,
    min_confidence: Option<String>,
}

fn load_deadcheck_config(path: &Path, cfg: &mut ProjectConfig) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return, // File not found — not required.
    };

    let dc: DeadcheckConfig = match serde_json::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: failed to parse {} — {e}", path.display());
            return;
        }
    };

    // Entry points in the config are relative to the project root.
    let root = cfg.root.clone();
    for entry in dc.entry_points {
        cfg.extra_entry_points.push(root.join(entry));
    }

    cfg.ignore_patterns.extend(dc.ignore);

    // Merge additional ignored dependencies.
    for dep in dc.ignore_dependencies {
        cfg.ignore_dependencies.insert(dep);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Default set of packages that are never imported in source files.
///
/// These are tooling-only packages that appear in `devDependencies` but are
/// configured through config files, not through `import` statements.
fn default_ignored_deps() -> HashSet<String> {
    [
        "typescript",
        "eslint",
        "prettier",
        "jest",
        "vitest",
        "ts-node",
        "tsx",
        "ts-jest",
        "@types/node",
        "@types/react",
        "@types/react-dom",
        "husky",
        "lint-staged",
        "commitizen",
        "semantic-release",
        "rimraf",
        "concurrently",
        "cross-env",
        "dotenv-cli",
        "nodemon",
        "turbo",
        "nx",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Strip `// line comments` and `/* block comments */` from JSON text.
///
/// `tsconfig.json` supports comments (JSONC format) but `serde_json` does not.
/// This is a simple regex-free stripping pass sufficient for tsconfig files.
fn strip_json_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }

        if in_string {
            if ch == '\\' {
                escaped = true;
                out.push(ch);
            } else if ch == '"' {
                in_string = false;
                out.push(ch);
            } else {
                out.push(ch);
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }

        // Check for `//` line comment.
        if ch == '/' && chars.peek() == Some(&'/') {
            // Consume until end of line.
            for c in chars.by_ref() {
                if c == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }

        // Check for `/* ... */` block comment.
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next(); // consume `*`
            let mut prev = ' ';
            for c in chars.by_ref() {
                if prev == '*' && c == '/' {
                    break;
                }
                if c == '\n' {
                    out.push('\n'); // preserve line count for error reporting
                }
                prev = c;
            }
            continue;
        }

        out.push(ch);
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_line_comments() {
        let input = r#"{ "key": "value" // comment
}"#;
        let stripped = strip_json_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn strips_block_comments() {
        let input = r#"{ /* block */ "key": "value" }"#;
        let stripped = strip_json_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn does_not_strip_url_in_string() {
        let input = r#"{ "url": "https://example.com" }"#;
        let stripped = strip_json_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(parsed["url"], "https://example.com");
    }

    #[test]
    fn framework_detection_nextjs() {
        let deps: HashSet<String> = ["next".to_string()].into();
        let dev: HashSet<String> = HashSet::new();
        assert_eq!(detect_framework(&deps, &dev), Framework::NextJs);
    }

    #[test]
    fn framework_detection_vite() {
        let deps: HashSet<String> = HashSet::new();
        let dev: HashSet<String> = ["vite".to_string()].into();
        assert_eq!(detect_framework(&deps, &dev), Framework::Vite);
    }
}
