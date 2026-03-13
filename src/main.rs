//! `deadcheck` — fast dead code detector for JavaScript and TypeScript projects.
//!
//! # Pipeline
//!
//! ```text
//! CLI args + config files
//!   └─► scanner   — find all JS/TS files in the project
//!         └─► parser    — extract imports & exports from each file (parallel)
//!               └─► graph     — build a directed dependency graph
//!                     └─► analyzer  — BFS reachability from entry points
//!                           └─► confidence — score each dead file
//!                                 └─► output — display / JSON / DOT
//!                                       └─► fix (optional) — safe-delete
//! ```

mod analyzer;
mod cli;
mod confidence;
mod config;
mod fix;
mod graph;
mod output;
mod parser;
mod resolver;
mod scanner;
mod types;
mod watch;

use anyhow::{Context, Result};
use clap::Parser as _;
use indicatif::{ProgressBar, ProgressStyle};

use cli::CliArgs;

fn main() -> Result<()> {
    let args = CliArgs::parse();

    let root = args
        .path
        .canonicalize()
        .with_context(|| format!("Cannot access directory: {}", args.path.display()))?;

    if args.watch {
        // Watch mode runs its own loop; it never returns unless there is an error.
        return watch::run(&root, &args);
    }

    run_analysis(&root, &args)
}

/// Run one complete analysis pass and produce output.
///
/// This is extracted so the watch loop can call it on every change.
pub fn run_analysis(root: &std::path::Path, args: &CliArgs) -> Result<()> {
    // ------------------------------------------------------------------
    // Load configuration (package.json, tsconfig.json, deadcheck.config.json)
    // ------------------------------------------------------------------
    let cfg = config::load(root, &args.entry, &args.ignore, args.config.as_deref())
        .context("Failed to load project configuration")?;

    // ------------------------------------------------------------------
    // Phase 1: Scan
    // ------------------------------------------------------------------
    let scan_spinner = spinner("Scanning files...");

    let files = scanner::scan(root, &cfg.ignore_patterns)
        .with_context(|| format!("Failed to scan project at {}", root.display()))?;

    scan_spinner.finish_and_clear();

    if files.is_empty() {
        eprintln!(
            "No JavaScript or TypeScript files found in {}",
            root.display()
        );
        return Ok(());
    }

    // ------------------------------------------------------------------
    // Phase 2: Parse (parallel)
    // ------------------------------------------------------------------
    let parse_bar = progress_bar(files.len() as u64, "Parsing {pos}/{len} files...");

    let file_infos = parser::parse_all(root, &files, &parse_bar).context("Parsing phase failed")?;

    parse_bar.finish_and_clear();

    // ------------------------------------------------------------------
    // Phase 3: Build dependency graph
    // ------------------------------------------------------------------
    let dep_graph =
        graph::build(root, file_infos, &cfg).context("Failed to build dependency graph")?;

    // ------------------------------------------------------------------
    // Phase 4: Analyze
    // ------------------------------------------------------------------
    let result = analyzer::analyze(&dep_graph, &cfg);

    // ------------------------------------------------------------------
    // Phase 5: Output
    // ------------------------------------------------------------------
    if args.json {
        output::print_json(&result)?;
    } else {
        output::print_terminal(&result, args.min_confidence, args.all);
    }

    if args.graph {
        output::write_dot(&dep_graph, root).context("Failed to write dependency graph")?;
    }

    // ------------------------------------------------------------------
    // Phase 6: Safe delete (--fix)
    // ------------------------------------------------------------------
    if args.fix {
        fix::apply(root, &result, args.min_confidence).context("Safe-delete operation failed")?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Progress indicator helpers (also used by watch.rs)
// ---------------------------------------------------------------------------

pub fn spinner(message: &str) -> ProgressBar {
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

pub fn progress_bar(len: u64, template: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template(&format!("{{spinner:.cyan}} {template}"))
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}
