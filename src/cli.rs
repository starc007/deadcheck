//! Command-line interface definition.
//!
//! All CLI flags and arguments are defined here using clap's derive API.
//! The rest of the application reads from [`CliArgs`] and never touches
//! `std::env` directly, making it easy to test the pipeline in isolation.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// Fast dead code detector for JavaScript and TypeScript projects.
#[derive(Debug, Parser)]
#[command(
    name = "deadcheck",
    version,
    author,
    about,
    long_about = "Scans a JS/TS project, builds a dependency graph, and reports \
                  files, exports, and npm packages that are never used."
)]
pub struct CliArgs {
    /// Root directory of the project to scan.
    ///
    /// Defaults to the current working directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Move dead files to `.deadcode/` instead of just reporting them.
    ///
    /// Files are never deleted — they are moved to a `.deadcode/` folder at
    /// the project root along with a `manifest.json` so you can undo the
    /// operation at any time.
    #[arg(long, short)]
    pub fix: bool,

    /// Output results as machine-readable JSON.
    #[arg(long, short)]
    pub json: bool,

    /// Export the dependency graph as a Graphviz DOT file (`dependency-graph.dot`).
    #[arg(long, short)]
    pub graph: bool,

    /// Watch for file changes and re-run the analysis automatically.
    #[arg(long, short)]
    pub watch: bool,

    /// Only show results at or above this confidence level.
    #[arg(long, short = 'c', value_name = "LEVEL", default_value = "low")]
    pub min_confidence: ConfidenceFilter,

    /// Override the entry points for the project.
    ///
    /// By default, entry points are detected automatically (e.g. `src/main.ts`,
    /// `pages/`, Next.js `app/`). Use this flag to add extra entry points on
    /// top of the auto-detected ones.
    #[arg(long, short, value_name = "FILE", num_args = 1..)]
    pub entry: Vec<PathBuf>,

    /// Additional glob patterns to exclude from the scan.
    ///
    /// Patterns use the same syntax as `.gitignore`. For example:
    /// `--ignore "src/generated/**"`.
    #[arg(long, short, value_name = "PATTERN", num_args = 1..)]
    pub ignore: Vec<String>,

    /// Path to a `deadcheck.config.json` configuration file.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,
}

/// Minimum confidence level to include in the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum ConfidenceFilter {
    /// Show only HIGH confidence results (safest to remove).
    High,
    /// Show HIGH and MEDIUM confidence results.
    Medium,
    /// Show all results, including LOW confidence ones (default).
    Low,
}
