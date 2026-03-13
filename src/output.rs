//! Output formatting for analysis results.
//!
//! Supports three output modes:
//!
//! 1. **Terminal** — human-readable, colour-coded output grouped by confidence.
//!    Long lists are truncated to [`MAX_SHOWN`] entries by default; pass
//!    `show_all = true` (via `--all`) to disable truncation.
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

/// Maximum number of entries shown per section before truncating.
const MAX_SHOWN: usize = 20;

// ---------------------------------------------------------------------------
// Terminal output
// ---------------------------------------------------------------------------

/// Print a human-readable report to stdout.
///
/// The summary line is printed **last** so it is always visible at the bottom
/// of the terminal after the command finishes, even on large projects.
pub fn print_terminal(result: &AnalysisResult, min_confidence: ConfidenceFilter, show_all: bool) {
    let min = filter_to_confidence(min_confidence);

    let dead: Vec<&DeadFile> = result
        .dead_files
        .iter()
        .filter(|f| f.confidence >= min)
        .collect();

    let nothing =
        dead.is_empty() && result.unused_exports.is_empty() && result.unused_dependencies.is_empty();

    if nothing {
        print_summary(result);
        println!("\n{}", "  No dead code found — everything looks reachable!".green().bold());
        println!();
        return;
    }

    // --- Dead files ---------------------------------------------------------
    if !dead.is_empty() {
        print_section_header("Dead Files", dead.len());

        let mut shown: usize = 0;

        for group_confidence in [Confidence::High, Confidence::Medium, Confidence::Low] {
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

            println!("  {}", confidence_badge(group_confidence));

            for file in &group {
                if !show_all && shown >= MAX_SHOWN {
                    break;
                }
                println!("    {} {}", "›".dimmed(), file.path.dimmed());
                shown += 1;
            }
        }

        let total = dead.len();
        if !show_all && shown < total {
            println!(
                "\n    {} {} more  (run with {} to see all)",
                "…".dimmed(),
                (total - shown).to_string().yellow(),
                "--all".cyan()
            );
        }
    }

    // --- Unused exports ------------------------------------------------------
    if !result.unused_exports.is_empty() {
        print_section_header("Unused Exports", result.unused_exports.len());

        let mut current_file = "";
        let mut shown: usize = 0;

        for export in &result.unused_exports {
            if !show_all && shown >= MAX_SHOWN {
                break;
            }
            if export.file_path != current_file {
                println!("  {} {}", "›".dimmed(), export.file_path.dimmed());
                current_file = &export.file_path;
            }
            println!("      {} {}", "·".dimmed(), export.symbol_name.yellow());
            shown += 1;
        }

        let total = result.unused_exports.len();
        if !show_all && shown < total {
            println!(
                "\n    {} {} more  (run with {} to see all)",
                "…".dimmed(),
                (total - shown).to_string().yellow(),
                "--all".cyan()
            );
        }
    }

    // --- Unused npm dependencies --------------------------------------------
    if !result.unused_dependencies.is_empty() {
        print_section_header("Unused Dependencies", result.unused_dependencies.len());

        for dep in &result.unused_dependencies {
            println!("    {} {}", "·".dimmed(), dep.yellow());
        }
    }

    // Summary is printed LAST so it's always visible in the terminal.
    print_summary(result);

    // Hint: suggest --fix if there are dead files.
    if !dead.is_empty() {
        println!(
            "  {}",
            "Run with --fix to safely move dead files to .deadcode/".dimmed()
        );
    }

    println!();
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

/// Print a section header with a separator line and item count.
fn print_section_header(title: &str, count: usize) {
    // e.g.  ── Dead Files ─────────────────  18 found
    let label = format!(" {} ", title).bold().to_string();
    let count_str = format!("  {} found", count);
    let separator = "─".repeat(46_usize.saturating_sub(title.len()));

    println!(
        "\n{}{}{}",
        format!("── {label}").dimmed(),
        separator.dimmed(),
        if count > 0 {
            count_str.yellow().to_string()
        } else {
            count_str.green().to_string()
        }
    );
}

/// Print the summary block (always shown at the bottom).
fn print_summary(result: &AnalysisResult) {
    let dead = result.dead_files.len();
    let exports = result.unused_exports.len();
    let deps = result.unused_dependencies.len();

    let line = "─".repeat(56);
    println!("\n{}", line.dimmed());
    println!(
        "  {}  {}  {}  {}",
        format!("{} files scanned", result.total_files).bold(),
        format_count_label(dead, "dead file"),
        format_count_label(exports, "unused export"),
        format_count_label(deps, "unused dep"),
    );
    println!("{}", line.dimmed());
}

fn format_count_label(n: usize, label: &str) -> String {
    let plural = if n == 1 { label.to_string() } else { format!("{label}s") };
    if n > 0 {
        format!("{} {}", n.to_string().red().bold(), plural.dimmed())
    } else {
        format!("{} {}", "0".green(), plural.dimmed())
    }
}

fn confidence_badge(c: Confidence) -> String {
    match c {
        Confidence::High => format!(
            "{}  {}",
            "HIGH".red().bold(),
            "— likely safe to remove".dimmed()
        ),
        Confidence::Medium => format!(
            "{}  {}",
            "MEDIUM".yellow().bold(),
            "— review before removing".dimmed()
        ),
        Confidence::Low => format!(
            "{}  {}",
            "LOW".white().bold(),
            "— may be dynamically referenced".dimmed()
        ),
    }
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
