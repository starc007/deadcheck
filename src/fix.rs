//! Safe-delete mode (`--fix`).
//!
//! Moves dead files to a `.deadcode/` folder at the project root rather than
//! deleting them. A `manifest.json` file is written alongside the moved files
//! so the operation can be fully reversed.
//!
//! # Directory layout after `--fix`
//!
//! ```text
//! .deadcode/
//!   OldNavbar.tsx
//!   legacy/
//!     auth.ts
//!   manifest.json
//! ```
//!
//! # Reversing the operation
//!
//! ```bash
//! cat .deadcode/manifest.json | jq -r '.entries[].original_path' \
//!   | xargs -I{} sh -c 'mkdir -p "$(dirname {})" && mv ".deadcode/$(basename {})" "{}"'
//! ```

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;

use crate::cli::ConfidenceFilter;
use crate::types::{AnalysisResult, Confidence};

// ---------------------------------------------------------------------------
// Manifest types
// ---------------------------------------------------------------------------

/// Written to `.deadcode/manifest.json` so the operation can be undone.
#[derive(Debug, Serialize)]
struct Manifest {
    /// ISO 8601 timestamp of when the fix was applied.
    timestamp: String,

    /// One entry per moved file.
    entries: Vec<ManifestEntry>,
}

#[derive(Debug, Serialize)]
struct ManifestEntry {
    /// Original path relative to the project root.
    original_path: String,

    /// Path inside `.deadcode/` where the file was placed.
    moved_to: String,

    /// Confidence level that led to the move.
    confidence: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Move dead files to `.deadcode/` in the project root.
///
/// Only files at or above `min_confidence` are moved. HIGH and MEDIUM
/// confidence files are moved by default (`--min-confidence low` is never
/// combined with `--fix` in practice, but is supported).
///
/// # Errors
///
/// Returns an error if the `.deadcode/` directory cannot be created or if
/// a file cannot be moved.
pub fn apply(root: &Path, result: &AnalysisResult, min_confidence: ConfidenceFilter) -> Result<()> {
    let min = filter_to_confidence(min_confidence);
    let deadcode_dir = root.join(".deadcode");

    let candidates: Vec<_> = result
        .dead_files
        .iter()
        .filter(|f| f.confidence >= min)
        .collect();

    if candidates.is_empty() {
        println!("{}", "No files to move — nothing to do.".dimmed());
        return Ok(());
    }

    // Create the .deadcode/ directory if it doesn't exist.
    fs::create_dir_all(&deadcode_dir)
        .context("Failed to create .deadcode/ directory")?;

    let mut entries: Vec<ManifestEntry> = Vec::new();

    for dead in &candidates {
        let source = root.join(&dead.path);

        if !source.exists() {
            eprintln!("Warning: skipping {} — file not found", dead.path);
            continue;
        }

        // Preserve the relative directory structure inside .deadcode/.
        let destination = deadcode_dir.join(&dead.path);

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create {}", parent.display()))?;
        }

        fs::rename(&source, &destination)
            .with_context(|| {
                format!("Cannot move {} → {}", source.display(), destination.display())
            })?;

        let moved_to = destination
            .strip_prefix(root)
            .unwrap_or(&destination)
            .display()
            .to_string();

        println!(
            "  {} {} {}",
            "moved".dimmed(),
            dead.path.yellow(),
            format!("→ {moved_to}").dimmed()
        );

        entries.push(ManifestEntry {
            original_path: dead.path.clone(),
            moved_to,
            confidence: format!("{}", dead.confidence),
        });
    }

    // Write the manifest.
    write_manifest(&deadcode_dir, entries)?;

    println!(
        "\n{} {} file(s) moved to {}",
        "✓".green().bold(),
        candidates.len().to_string().cyan(),
        ".deadcode/".cyan()
    );
    println!(
        "  {}",
        "Run `cat .deadcode/manifest.json` to see what was moved.".dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_manifest(deadcode_dir: &Path, entries: Vec<ManifestEntry>) -> Result<()> {
    let manifest = Manifest {
        timestamp: current_timestamp(),
        entries,
    };

    let json = serde_json::to_string_pretty(&manifest)
        .context("Failed to serialise manifest")?;

    let manifest_path = deadcode_dir.join("manifest.json");
    fs::write(&manifest_path, json)
        .with_context(|| format!("Cannot write {}", manifest_path.display()))?;

    Ok(())
}

/// Returns a basic ISO 8601-style timestamp using only `std`.
///
/// We avoid pulling in a `chrono` / `time` dependency just for this one string.
/// The format is `YYYY-MM-DDThh:mm:ssZ` — precise enough for a manifest.
fn current_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Very simple conversion — no leap-second handling needed.
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400; // days since epoch

    // Rata Die algorithm to get year/month/day from days since 1970-01-01.
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from https://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn filter_to_confidence(filter: ConfidenceFilter) -> Confidence {
    match filter {
        ConfidenceFilter::High => Confidence::High,
        ConfidenceFilter::Medium => Confidence::Medium,
        ConfidenceFilter::Low => Confidence::Low,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_format() {
        let ts = current_timestamp();
        // Basic sanity: should be 20 chars like "2024-03-15T10:22:05Z"
        assert_eq!(ts.len(), 20, "timestamp should be 20 chars: {ts}");
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
    }

    #[test]
    fn days_to_ymd_epoch() {
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2000-01-01 = 10957 days since Unix epoch (1970-01-01).
        // 30 years × 365 + 7 leap years = 10950 + 7 = 10957.
        let (y, m, d) = days_to_ymd(10957);
        assert_eq!((y, m, d), (2000, 1, 1));
    }
}
