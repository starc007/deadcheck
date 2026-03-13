//! `deadcheck` — fast dead code detector for JavaScript and TypeScript projects.
//!
//! # Pipeline
//!
//! ```text
//! CLI args
//!   └─► scanner   — find all JS/TS files in the project
//!         └─► parser    — extract imports & exports from each file (parallel)
//!               └─► resolver  — turn specifiers into absolute paths
//!                     └─► graph     — build a directed dependency graph
//!                           └─► analyzer  — BFS reachability from entry points
//!                                 └─► confidence — score each dead file
//!                                       └─► output — display results
//! ```

mod analyzer;
mod cli;
mod confidence;
mod graph;
mod output;
mod parser;
mod resolver;
mod scanner;
mod types;

use anyhow::{Context, Result};
use clap::Parser as _;
use indicatif::{ProgressBar, ProgressStyle};

use cli::CliArgs;

fn main() -> Result<()> {
    let args = CliArgs::parse();

    // Resolve the project root to an absolute path so every downstream
    // module can work with canonical paths.
    let root = args
        .path
        .canonicalize()
        .with_context(|| format!("Cannot access directory: {}", args.path.display()))?;

    // ------------------------------------------------------------------
    // Phase 1: Scan
    // ------------------------------------------------------------------
    let scan_spinner = spinner("Scanning files...");

    let files = scanner::scan(&root, &args.ignore)
        .with_context(|| format!("Failed to scan project at {}", root.display()))?;

    scan_spinner.finish_and_clear();

    if files.is_empty() {
        eprintln!("No JavaScript or TypeScript files found in {}", root.display());
        return Ok(());
    }

    // ------------------------------------------------------------------
    // Phase 2: Parse (parallel)
    // ------------------------------------------------------------------
    let parse_bar = progress_bar(files.len() as u64, "Parsing {pos}/{len} files...");

    let file_infos = parser::parse_all(&root, &files, &parse_bar)
        .context("Parsing phase failed")?;

    parse_bar.finish_and_clear();

    // ------------------------------------------------------------------
    // Phase 3: Build dependency graph
    // ------------------------------------------------------------------
    let graph = graph::build(&root, file_infos, &args.entry)
        .context("Failed to build dependency graph")?;

    // ------------------------------------------------------------------
    // Phase 4: Analyze
    // ------------------------------------------------------------------
    let result = analyzer::analyze(&graph, &root);

    // ------------------------------------------------------------------
    // Phase 5: Output
    // ------------------------------------------------------------------
    if args.json {
        output::print_json(&result)?;
    } else {
        output::print_terminal(&result, args.min_confidence);
    }

    if args.graph {
        output::write_dot(&graph, &root).context("Failed to write dependency graph")?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}

fn progress_bar(len: u64, template: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template(&format!("{{spinner:.cyan}} {template}"))
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}
