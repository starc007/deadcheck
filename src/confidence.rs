//! Confidence scoring for dead file candidates.
//!
//! Each file that is unreachable from entry points receives a **score** from 0
//! to 100 based on a set of weighted signals. The score is then bucketed into
//! one of three confidence levels:
//!
//! | Score     | Level  |
//! |-----------|--------|
//! | 75 – 100  | HIGH   |
//! | 40 – 74   | MEDIUM |
//! | 0  – 39   | LOW    |
//!
//! The signal table is intentionally visible and documented so contributors can
//! tune weights without having to trace through logic.

use crate::graph::DependencyGraph;
use crate::types::{Confidence, ConfidenceSignal, ExportKind, FileInfo, SignalKind};

// ---------------------------------------------------------------------------
// Score thresholds
// ---------------------------------------------------------------------------

const HIGH_THRESHOLD: i32 = 75;
const MEDIUM_THRESHOLD: i32 = 40;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute a confidence score for an unreachable file.
///
/// Returns the bucketed [`Confidence`] level and the individual signals that
/// contributed to the score (useful for explaining results to the user).
///
/// # Arguments
///
/// * `file` — the file being evaluated.
/// * `graph` — the full dependency graph (used for structural checks).
/// * `incoming_count` — number of graph edges pointing *into* this file.
/// * `has_dynamic_import_incoming` — whether any dynamic `import()` edge
///   points at this file (reduces confidence because it may be loaded at
///   runtime).
pub fn score(
    file: &FileInfo,
    // Reserved for Phase 3 structural checks (e.g. barrel file detection).
    _graph: &DependencyGraph,
    incoming_count: usize,
    has_dynamic_import_incoming: bool,
) -> (Confidence, Vec<ConfidenceSignal>) {
    let mut signals: Vec<ConfidenceSignal> = Vec::new();

    // --- Positive signals (increase confidence that the file is dead) -------

    if incoming_count == 0 {
        signals.push(signal(SignalKind::NotImportedByAnyFile));
    }

    // The file is not an entry point (already confirmed by the caller, since
    // only non-entry files are scored).
    signals.push(signal(SignalKind::NotAnEntryPoint));

    let path_str = file.relative_path.to_string_lossy();

    if !matches_framework_pattern(&path_str) {
        signals.push(signal(SignalKind::NoMatchingFrameworkPattern));
    }

    // --- Negative signals (decrease confidence) -----------------------------

    if has_dynamic_import_incoming {
        signals.push(signal(SignalKind::HasDynamicImportReferring));
    }

    if is_barrel_file(file) {
        signals.push(signal(SignalKind::IsBarrelFile));
    }

    if matches_route_pattern(&path_str) {
        signals.push(signal(SignalKind::MatchesRoutePattern));
    }

    if is_test_file(&path_str) {
        signals.push(signal(SignalKind::IsTestFile));
    }

    if has_export_star(file) {
        signals.push(signal(SignalKind::HasExportStar));
    }

    if in_public_directory(&path_str) {
        signals.push(signal(SignalKind::InPublicDirectory));
    }

    // Compute the raw score and clamp to [0, 100].
    let raw: i32 = signals.iter().map(|s| s.delta).sum();
    let clamped = raw.clamp(0, 100);

    let confidence = if clamped >= HIGH_THRESHOLD {
        Confidence::High
    } else if clamped >= MEDIUM_THRESHOLD {
        Confidence::Medium
    } else {
        Confidence::Low
    };

    (confidence, signals)
}

// ---------------------------------------------------------------------------
// Signal helpers
// ---------------------------------------------------------------------------

/// Construct a [`ConfidenceSignal`] from a [`SignalKind`], using the kind's
/// predefined delta value.
fn signal(kind: SignalKind) -> ConfidenceSignal {
    ConfidenceSignal {
        delta: kind.delta(),
        kind,
    }
}

// ---------------------------------------------------------------------------
// Individual signal checks
// ---------------------------------------------------------------------------

/// Returns `true` if the file only contains re-export declarations (barrel).
///
/// A barrel file exclusively re-exports symbols from other modules and has no
/// own declarations. These are commonly used as public API surfaces and should
/// be treated with lower confidence.
fn is_barrel_file(file: &FileInfo) -> bool {
    // A barrel has no dynamic imports, no own logic, and all exports are
    // re-exports (`export * from "..."` or `export { x } from "..."`).
    if file.imports.is_empty() && file.exports.is_empty() {
        return false; // Empty file — not a barrel.
    }

    // All exports must be star re-exports or the file name must be `index.*`.
    let all_reexports = file
        .exports
        .iter()
        .all(|e| e.kind == ExportKind::ReExportAll);

    let is_index = file
        .relative_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s == "index")
        .unwrap_or(false);

    all_reexports || (is_index && !file.exports.is_empty())
}

/// Returns `true` if the file path looks like a framework route.
///
/// This applies the same heuristics as the entry point detector so that files
/// close to routes get lower confidence even if not directly detected.
fn matches_route_pattern(path: &str) -> bool {
    let path_lower = path.to_lowercase();

    // Next.js App Router special files.
    let app_route_stems = [
        "page", "layout", "route", "loading", "error", "not-found", "template",
    ];
    if path_lower.contains("/app/") || path_lower.starts_with("app/") {
        for stem in &app_route_stems {
            if path_lower.contains(&format!("/{stem}.")) {
                return true;
            }
        }
    }

    // Next.js Pages Router.
    if path_lower.starts_with("pages/") {
        return true;
    }

    // Remix routes.
    if path_lower.contains("/routes/") {
        return true;
    }

    false
}

/// Returns `true` if the path looks like a framework route for the
/// *positive* signal (i.e. it does NOT match any framework pattern).
fn matches_framework_pattern(path: &str) -> bool {
    matches_route_pattern(path)
}

/// Returns `true` if the file is a test or spec file.
fn is_test_file(path: &str) -> bool {
    let path_lower = path.to_lowercase();
    path_lower.contains(".test.")
        || path_lower.contains(".spec.")
        || path_lower.contains("/__tests__/")
        || path_lower.contains("/test/")
        || path_lower.contains("/tests/")
}

/// Returns `true` if any export in the file is a `export *` star re-export.
fn has_export_star(file: &FileInfo) -> bool {
    file.exports
        .iter()
        .any(|e| e.kind == ExportKind::ReExportAll)
}

/// Returns `true` if the file lives in a `public/` or `assets/` directory.
fn in_public_directory(path: &str) -> bool {
    let path_lower = path.to_lowercase();
    path_lower.starts_with("public/")
        || path_lower.contains("/public/")
        || path_lower.starts_with("assets/")
        || path_lower.contains("/assets/")
        || path_lower.starts_with("static/")
        || path_lower.contains("/static/")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_detection() {
        assert!(is_test_file("src/auth.test.ts"));
        assert!(is_test_file("src/__tests__/utils.ts"));
        assert!(!is_test_file("src/utils.ts"));
    }

    #[test]
    fn route_pattern_detection() {
        assert!(matches_route_pattern("app/dashboard/page.tsx"));
        assert!(matches_route_pattern("pages/index.tsx"));
        assert!(matches_route_pattern("app/routes/home.tsx"));
        assert!(!matches_route_pattern("src/components/Button.tsx"));
    }

    #[test]
    fn public_directory_detection() {
        assert!(in_public_directory("public/workers/service-worker.js"));
        assert!(in_public_directory("src/assets/utils.js"));
        assert!(!in_public_directory("src/components/Button.tsx"));
    }

    #[test]
    fn score_thresholds() {
        assert_eq!(HIGH_THRESHOLD, 75);
        assert_eq!(MEDIUM_THRESHOLD, 40);
    }
}
