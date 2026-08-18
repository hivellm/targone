//! `cargo targone` — the Targone engine CLI. Phase 1 scope: read-only
//! discovery and reporting. Nothing here deletes anything.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use targone_core::{discover, scan_target_dir, TargetReport};

#[derive(Parser)]
#[command(
    name = "cargo-targone",
    bin_name = "cargo targone",
    version,
    about = "target/, gone — bounded disk usage for Rust build directories"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan roots for target dirs and report sizes and reclaimable bytes per tier.
    Report {
        /// Directories to scan (defaults to the current directory).
        paths: Vec<PathBuf>,
        /// Emit the full report as JSON on stdout.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    // Cargo invokes external subcommands as `cargo-targone targone <args…>`;
    // drop the duplicated subcommand name so both invocation styles parse.
    let args = std::env::args_os()
        .enumerate()
        .filter(|(i, a)| !(*i == 1 && a == "targone"))
        .map(|(_, a)| a);
    let cli = Cli::parse_from(args);
    match cli.command {
        Command::Report { paths, json } => report(paths, json),
    }
}

fn report(mut paths: Vec<PathBuf>, json: bool) -> ExitCode {
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let dirs = discover(&paths);
    let reports: Vec<TargetReport> = dirs.iter().map(scan_target_dir).collect();

    if json {
        match serde_json::to_string_pretty(&reports) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: failed to serialize report: {e}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    if reports.is_empty() {
        println!("No target directories found under the given paths.");
        return ExitCode::SUCCESS;
    }

    let mut total = 0u64;
    let mut reclaimable = 0u64;
    let mut rows: Vec<(u64, u64, String)> = reports
        .iter()
        .map(|r| {
            let t = r.total_bytes();
            let c = r.reclaimable_bytes();
            (t, c, r.root.display().to_string())
        })
        .collect();
    // Descending by reclaimable bytes — the order a budget-driven sweep
    // would process them (F-001: heavy-tail distribution).
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));

    println!("{:>10}  {:>12}  target dir", "total", "reclaimable");
    for (t, c, root) in &rows {
        total += t;
        reclaimable += c;
        println!("{:>10}  {:>12}  {}", human(*t), human(*c), root);
    }
    println!(
        "{:>10}  {:>12}  TOTAL ({} dirs)",
        human(total),
        human(reclaimable),
        rows.len()
    );

    for r in &reports {
        for p in &r.profiles {
            if p.unparsed_kept > 0 {
                eprintln!(
                    "note: {} entries in {} matched no known grammar and were kept (fail-open)",
                    p.unparsed_kept,
                    p.path.display()
                );
            }
        }
    }
    ExitCode::SUCCESS
}

fn human(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
