//! `cargo targone` — the Targone engine CLI.
//!
//! `report` is read-only. `gc` is dry-run by DEFAULT; only `--apply` deletes,
//! and then only under Cargo's lock protocol with an append-only audit log.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use targone_core::{discover, scan_target_dir, sweep_profile, SweepOutcome, TargetReport};

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
    /// Reclaim superseded artifacts. DRY RUN unless --apply is passed.
    Gc {
        /// Directories to scan (defaults to the current directory).
        paths: Vec<PathBuf>,
        /// Actually delete. Without this flag, gc only prints what it would do.
        #[arg(long)]
        apply: bool,
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
        Command::Gc { paths, apply } => gc(paths, apply),
    }
}

fn scan_all(mut paths: Vec<PathBuf>) -> Vec<TargetReport> {
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let dirs = discover(&paths);
    let mut reports: Vec<TargetReport> = dirs.iter().map(scan_target_dir).collect();
    // Descending by reclaimable bytes — the order a budget-driven sweep
    // processes them (F-001: heavy-tail distribution).
    reports.sort_by_key(|r| std::cmp::Reverse(r.reclaimable_bytes()));
    reports
}

fn report(paths: Vec<PathBuf>, json: bool) -> ExitCode {
    let reports = scan_all(paths);
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
    print_table(&reports);
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

fn print_table(reports: &[TargetReport]) {
    let mut total = 0u64;
    let mut reclaimable = 0u64;
    println!("{:>10}  {:>12}  target dir", "total", "reclaimable");
    for r in reports {
        let t = r.total_bytes();
        let c = r.reclaimable_bytes();
        total += t;
        reclaimable += c;
        println!("{:>10}  {:>12}  {}", human(t), human(c), r.root.display());
    }
    println!(
        "{:>10}  {:>12}  TOTAL ({} dirs)",
        human(total),
        human(reclaimable),
        reports.len()
    );
}

fn gc(paths: Vec<PathBuf>, apply: bool) -> ExitCode {
    let reports = scan_all(paths);
    if reports.is_empty() {
        println!("No target directories found under the given paths.");
        return ExitCode::SUCCESS;
    }
    if !apply {
        print_table(&reports);
        println!("\nDRY RUN — nothing was deleted. Pass --apply to reclaim.");
        return ExitCode::SUCCESS;
    }

    let run_id = format!(
        "gc-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let mut audit = match open_audit_log() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: cannot open audit log: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut freed = 0u64;
    let mut residue = 0u64;
    let mut skipped_locked = 0u64;
    for r in &reports {
        let mut dir_freed = 0u64;
        let mut notes: Vec<String> = Vec::new();
        for p in &r.profiles {
            match sweep_profile(p, &run_id, &mut audit) {
                Ok(SweepOutcome {
                    freed_bytes,
                    residue_paths,
                    skipped_locked: locked,
                    refused,
                    items_skipped,
                    ..
                }) => {
                    dir_freed += freed_bytes;
                    residue += residue_paths;
                    if locked {
                        skipped_locked += 1;
                        notes.push(format!("{}: build in progress, skipped", p.path.display()));
                    }
                    if let Some(reason) = refused {
                        notes.push(format!("{}: refused ({reason})", p.path.display()));
                    }
                    if items_skipped > 0 {
                        notes.push(format!(
                            "{}: {} items skipped (session locks)",
                            p.path.display(),
                            items_skipped
                        ));
                    }
                }
                Err(e) => notes.push(format!("{}: error: {e}", p.path.display())),
            }
        }
        freed += dir_freed;
        println!("{:>10} freed  {}", human(dir_freed), r.root.display());
        for n in notes {
            println!("            note: {n}");
        }
    }
    println!(
        "{:>10} freed  TOTAL ({} dirs; {} profiles skipped by live builds; {} residue paths)",
        human(freed),
        reports.len(),
        skipped_locked,
        residue
    );
    let _ = audit.flush();
    ExitCode::SUCCESS
}

/// Append-only audit log at `$CARGO_HOME/targone/audit.jsonl`.
fn open_audit_log() -> std::io::Result<impl Write> {
    let dir = cargo_home().join("targone");
    fs::create_dir_all(&dir)?;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("audit.jsonl"))
}

fn cargo_home() -> PathBuf {
    if let Some(h) = std::env::var_os("CARGO_HOME") {
        return PathBuf::from(h);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cargo")
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
