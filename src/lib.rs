//! Public library interface for `deadcheck`.
//!
//! The binary (`src/main.rs`) is the primary consumer of these modules, but
//! exporting them as a library allows integration tests and downstream tooling
//! to run the pipeline programmatically without shelling out.

pub mod analyzer;
pub mod cli;
pub mod confidence;
pub mod config;
pub mod fix;
pub mod graph;
pub mod output;
pub mod parser;
pub mod resolver;
pub mod scanner;
pub mod types;
pub mod watch;

pub use main_impl::{progress_bar, run_analysis, spinner};

// Re-export the pipeline runner from main so tests can call it.
mod main_impl {
    use std::path::Path;

    use anyhow::Result;
    use indicatif::{ProgressBar, ProgressStyle};

    use crate::cli::CliArgs;
    use crate::{analyzer, config, fix, graph, output, parser, scanner};

    pub fn run_analysis(root: &Path, args: &CliArgs) -> Result<()> {
        let cfg = config::load(root, &args.entry, &args.ignore, args.config.as_deref())?;
        let files = scanner::scan(root, &cfg.ignore_patterns)?;

        if files.is_empty() {
            return Ok(());
        }

        let pb = progress_bar(files.len() as u64, "Parsing {pos}/{len} files...");
        let file_infos = parser::parse_all(root, &files, &pb)?;
        pb.finish_and_clear();

        let dep_graph = graph::build(root, file_infos, &cfg)?;
        let result = analyzer::analyze(&dep_graph, &cfg);

        if args.json {
            output::print_json(&result)?;
        } else {
            output::print_terminal(&result, args.min_confidence, args.all);
        }

        if args.graph {
            output::write_dot(&dep_graph, root)?;
        }

        if args.fix {
            fix::apply(root, &result, args.min_confidence)?;
        }

        Ok(())
    }

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
}
