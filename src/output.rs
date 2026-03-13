//! Output formatting for analysis results.
//!
//! Supports three output modes:
//!
//! 1. **Terminal** — human-readable, colour-coded output grouped by confidence.
//! 2. **JSON** — machine-readable via `--json`, serialised with `serde_json`.
//! 3. **DOT** — Graphviz DOT format for the dependency graph via `--graph`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use petgraph::dot::{Config, Dot};

use crate::cli::ConfidenceFilter;
use crate::graph::DependencyGraph;
use crate::types::{AnalysisResult, Confidence, DeadFile};

// ---------------------------------------------------------------------------
// Terminal output
// ---------------------------------------------------------------------------

/// Print a human-readable report to stdout.
pub fn print_terminal(result: &AnalysisResult, min_confidence: ConfidenceFilter) {
    let min = filter_to_confidence(min_confidence);

    print_summary(result);

    let dead: Vec<&DeadFile> = result
        .dead_files
        .iter()
        .filter(|f| f.confidence >= min)
        .collect();

    if dead.is_empty() && result.unused_exports.is_empty() && result.unused_dependencies.is_empty()
    {
        println!("\n{}", "No dead code found.".green().bold());
        return;
    }

    // --- Dead files ---------------------------------------------------------
    if !dead.is_empty() {
        println!("\n{}", "Dead Files".bold().underline());

        for group_confidence in [Confidence::High, Confidence::Medium, Confidence::Low] {
            // Skip groups below the minimum threshold.
            // Confidence ordering: Low < Medium < High.
            if group_confidence < min {
                continue;
            }

            let group: Vec<&&DeadFile> = dead
                .iter()
                .filter(|f| f.confidence == group_confidence)
                .collect();

            if group.is_empty() {
                continue;
            }

            println!("\n  {}", format_confidence_label(group_confidence));

            for file in group {
                println!("  {} {}", bullet(), file.path.dimmed());
            }
        }
    }

    // --- Unused exports ------------------------------------------------------
    if !result.unused_exports.is_empty() {
        println!("\n{}", "Unused Exports".bold().underline());

        // Group by file for readability.
        let mut current_file = "";
        for export in &result.unused_exports {
            if export.file_path != current_file {
                println!("\n  {}", export.file_path.dimmed());
                current_file = &export.file_path;
            }
            println!("    {} {}", bullet(), export.symbol_name.yellow());
        }
    }

    // --- Unused npm dependencies --------------------------------------------
    if !result.unused_dependencies.is_empty() {
        println!("\n{}", "Unused npm Dependencies".bold().underline());
        println!(
            "  {}",
            "(not imported in any source file)".italic().dimmed()
        );
        for dep in &result.unused_dependencies {
            println!("  {} {}", bullet(), dep.yellow());
        }
    }

    println!();
}

/// Print the summary line at the top of the report.
fn print_summary(result: &AnalysisResult) {
    let dead_count = result.dead_files.len();
    let export_count = result.unused_exports.len();
    let dep_count = result.unused_dependencies.len();

    println!("\n{}", "Dead Code Analysis".bold());
    println!(
        "  Scanned {} files  •  {} dead files  •  {} unused exports  •  {} unused dependencies",
        result.total_files.to_string().cyan(),
        format_count(dead_count, "red"),
        format_count(export_count, "yellow"),
        format_count(dep_count, "yellow"),
    );
}

fn format_count(n: usize, colour: &str) -> String {
    let s = n.to_string();
    match colour {
        "red" => {
            if n > 0 {
                s.red().to_string()
            } else {
                s.green().to_string()
            }
        }
        "yellow" => {
            if n > 0 {
                s.yellow().to_string()
            } else {
                s.green().to_string()
            }
        }
        _ => s,
    }
}

fn format_confidence_label(c: Confidence) -> String {
    match c {
        Confidence::High => "HIGH confidence".red().bold().to_string(),
        Confidence::Medium => "MEDIUM confidence".yellow().bold().to_string(),
        Confidence::Low => "LOW confidence".white().bold().to_string(),
    }
}

fn bullet() -> colored::ColoredString {
    "•".dimmed()
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

/// Serialise the analysis result as pretty-printed JSON to stdout.
pub fn print_json(result: &AnalysisResult) -> Result<()> {
    let json =
        serde_json::to_string_pretty(result).context("Failed to serialise result as JSON")?;
    println!("{json}");
    Ok(())
}

// ---------------------------------------------------------------------------
// DOT graph output
// ---------------------------------------------------------------------------

/// Write a Graphviz DOT file representing the dependency graph.
///
/// The output file is named `dependency-graph.dot` in the project root.
pub fn write_dot(graph: &DependencyGraph, root: &Path) -> Result<()> {
    let output_path = root.join("dependency-graph.dot");

    let dot = format!(
        "{:?}",
        Dot::with_config(&graph.graph, &[Config::EdgeNoLabel])
    );

    fs::write(&output_path, dot)
        .with_context(|| format!("Cannot write {}", output_path.display()))?;

    println!(
        "{} {}",
        "Dependency graph written to".dimmed(),
        output_path.display().to_string().cyan()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn filter_to_confidence(filter: ConfidenceFilter) -> Confidence {
    match filter {
        ConfidenceFilter::High => Confidence::High,
        ConfidenceFilter::Medium => Confidence::Medium,
        ConfidenceFilter::Low => Confidence::Low,
    }
}
