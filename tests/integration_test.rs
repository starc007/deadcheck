//! Integration tests for the full deadcheck pipeline.
//!
//! Each test runs the complete scan → parse → graph → analyze pipeline
//! against a small fixture project and asserts on the results.

use std::path::Path;

// Re-use the pipeline modules directly so we test the real code, not just
// the binary output.
use deadcheck::{analyzer, config, graph, parser, scanner};

fn run_fixture(fixture_name: &str) -> deadcheck::types::AnalysisResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture_name);

    assert!(root.exists(), "Fixture not found: {}", root.display());

    let cfg = config::load(&root, &[], &[], None).expect("config load failed");
    let files = scanner::scan(&root, &cfg.ignore_patterns).expect("scan failed");

    // Use a no-op progress bar for tests.
    let pb = indicatif::ProgressBar::hidden();
    let file_infos = parser::parse_all(&root, &files, &pb).expect("parse failed");

    let dep_graph = graph::build(&root, file_infos, &cfg).expect("graph build failed");

    analyzer::analyze(&dep_graph, &cfg)
}

// ---------------------------------------------------------------------------
// simple fixture
// ---------------------------------------------------------------------------

#[test]
fn simple_detects_dead_file() {
    let result = run_fixture("simple");

    let dead_paths: Vec<&str> = result.dead_files.iter().map(|f| f.path.as_str()).collect();

    assert!(
        dead_paths.iter().any(|p| p.contains("dead.ts")),
        "expected dead.ts to be flagged as dead, got: {dead_paths:?}"
    );
}

#[test]
fn simple_does_not_flag_entry_point() {
    let result = run_fixture("simple");

    let dead_paths: Vec<&str> = result.dead_files.iter().map(|f| f.path.as_str()).collect();

    assert!(
        !dead_paths.iter().any(|p| p.contains("index.ts")),
        "index.ts should not be flagged as dead: {dead_paths:?}"
    );
}

#[test]
fn simple_does_not_flag_imported_file() {
    let result = run_fixture("simple");

    let dead_paths: Vec<&str> = result.dead_files.iter().map(|f| f.path.as_str()).collect();

    assert!(
        !dead_paths.iter().any(|p| p.contains("utils.ts")),
        "utils.ts is imported and should not be dead: {dead_paths:?}"
    );
}

#[test]
fn simple_detects_unused_export() {
    let result = run_fixture("simple");

    let unused: Vec<&str> = result
        .unused_exports
        .iter()
        .map(|e| e.symbol_name.as_str())
        .collect();

    assert!(
        unused.contains(&"legacyHelper"),
        "expected legacyHelper to be flagged, got: {unused:?}"
    );
}

#[test]
fn simple_detects_unused_dependency() {
    let result = run_fixture("simple");

    assert!(
        result.unused_dependencies.contains(&"unused-package".to_string()),
        "expected unused-package to be flagged, got: {:?}",
        result.unused_dependencies
    );
}

#[test]
fn simple_flags_all_unused_dependencies() {
    // The fixture imports neither `react` nor `unused-package`, so both
    // should appear as unused.
    let result = run_fixture("simple");

    assert!(
        result.unused_dependencies.contains(&"unused-package".to_string()),
        "unused-package should be flagged: {:?}",
        result.unused_dependencies
    );
    assert!(
        result.unused_dependencies.contains(&"react".to_string()),
        "react is not imported in this fixture, so it should also be flagged: {:?}",
        result.unused_dependencies
    );
}

#[test]
fn simple_file_counts_are_consistent() {
    let result = run_fixture("simple");

    assert_eq!(
        result.total_files,
        result.reachable_count + result.dead_files.len(),
        "total_files should equal reachable + dead"
    );
}
