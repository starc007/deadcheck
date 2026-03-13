//! Watch mode (`--watch`).
//!
//! Runs the full analysis pipeline once immediately, then watches the project
//! directory for file changes. On any `.js` / `.ts` / `.jsx` / `.tsx` change,
//! clears the terminal and re-runs the analysis.
//!
//! Uses [`notify_debouncer_mini`] so that rapid bursts of file events (e.g.
//! a formatter saving multiple files at once) are collapsed into a single
//! re-run after a short debounce delay.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use colored::Colorize;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, DebounceEventResult};

use crate::cli::CliArgs;

/// JS/TS extensions that trigger a re-run when changed.
const WATCHED_EXTENSIONS: &[&str] = &["js", "ts", "jsx", "tsx", "mjs", "cjs", "mts", "cts"];

/// How long to wait after the last event before re-running (milliseconds).
const DEBOUNCE_MS: u64 = 300;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Enter watch mode: run once, then re-run on every relevant file change.
///
/// This function blocks indefinitely until the user presses Ctrl-C.
pub fn run(root: &Path, args: &CliArgs) -> Result<()> {
    println!(
        "{} {} {}",
        "●".cyan().bold(),
        "Watch mode".bold(),
        format!("— watching {}", root.display()).dimmed()
    );
    println!("{}\n", "Press Ctrl-C to stop.".dimmed());

    // Run the initial analysis immediately.
    run_and_print(root, args);

    // Set up the filesystem watcher.
    let (tx, rx) = mpsc::channel::<DebounceEventResult>();

    let mut debouncer = new_debouncer(Duration::from_millis(DEBOUNCE_MS), tx)
        .map_err(|e| anyhow::anyhow!("Failed to create file watcher: {e}"))?;

    debouncer
        .watcher()
        .watch(root, RecursiveMode::Recursive)
        .map_err(|e| anyhow::anyhow!("Failed to watch {}: {e}", root.display()))?;

    // Event loop.
    for event_result in &rx {
        match event_result {
            Ok(events) => {
                // Only re-run if at least one changed file is a JS/TS source file.
                let relevant = events.iter().any(|e| {
                    e.path.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| WATCHED_EXTENSIONS.contains(&ext))
                        .unwrap_or(false)
                });

                if relevant {
                    clear_screen();
                    println!(
                        "\n{} {}\n",
                        "↺".cyan().bold(),
                        "File changed — re-running analysis...".dimmed()
                    );
                    run_and_print(root, args);
                }
            }
            Err(e) => {
                eprintln!("Watch error: {e:?}");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run one analysis pass, printing errors to stderr without exiting.
///
/// Watch mode should never crash on a transient parse error (e.g. mid-save
/// incomplete file) — we just print the error and wait for the next event.
fn run_and_print(root: &Path, args: &CliArgs) {
    if let Err(e) = crate::run_analysis(root, args) {
        eprintln!("{} {e:#}", "Error:".red().bold());
    }
}

/// Clear the terminal screen using ANSI escape codes.
fn clear_screen() {
    // \x1B[2J clears the screen; \x1B[H moves cursor to top-left.
    print!("\x1B[2J\x1B[H");
}
