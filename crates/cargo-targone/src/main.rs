//! `cargo targone` — the Targone engine CLI.
//!
//! `report` is read-only. `gc` is dry-run by DEFAULT; only `--apply` deletes,
//! and then only under Cargo's lock protocol with an append-only audit log.
//! `scan` adopts projects into the machine registry; `schedule` wires the OS
//! scheduler for set-and-forget recurrence.

mod config;
mod schedule;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use targone_core::{
    discover, scan_target_dir, select_for_budget, sweep_profile, Registry, SweepOutcome,
    TargetReport,
};

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
        /// Restrict to specific tiers (repeatable). Default: all tiers.
        #[arg(long, value_enum)]
        tier: Vec<TierArg>,
        /// Emit the outcome summary as JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Find projects under the given roots and adopt them into the registry.
    Scan {
        /// Roots to search for Cargo projects.
        roots: Vec<PathBuf>,
    },
    /// Manage the OS-scheduled recurring sweep.
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
}

#[derive(Subcommand)]
enum ScheduleAction {
    /// Register the per-user scheduled task/timer (idempotent).
    Install,
    /// Remove the scheduled task/timer.
    Uninstall,
    /// Show scheduler state and the last scheduled run.
    Status,
    /// Execute one scheduled sweep (what the scheduler invokes).
    Run,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum TierArg {
    Incremental,
    Units,
    BuildScripts,
    OrphanFingerprints,
}

impl From<TierArg> for targone_core::Tier {
    fn from(t: TierArg) -> Self {
        match t {
            TierArg::Incremental => targone_core::Tier::Incremental,
            TierArg::Units => targone_core::Tier::Units,
            TierArg::BuildScripts => targone_core::Tier::BuildScripts,
            TierArg::OrphanFingerprints => targone_core::Tier::OrphanFingerprints,
        }
    }
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
        Command::Gc {
            paths,
            apply,
            tier,
            json,
        } => gc(paths, apply, &tier, json),
        Command::Scan { roots } => scan_cmd(roots),
        Command::Schedule { action } => match action {
            ScheduleAction::Install => print_result(schedule::install()),
            ScheduleAction::Uninstall => print_result(schedule::uninstall()),
            ScheduleAction::Status => schedule_status(),
            ScheduleAction::Run => scheduled_run(),
        },
    }
}

fn print_result(r: Result<String, String>) -> ExitCode {
    match r {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
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

/// Restrict every profile's plan (and derived estimates) to the given tiers.
fn filter_tiers(reports: &mut [TargetReport], tiers: &[TierArg]) {
    if tiers.is_empty() {
        return;
    }
    let selected: Vec<targone_core::Tier> = tiers.iter().map(|&t| t.into()).collect();
    for r in reports.iter_mut() {
        for p in &mut r.profiles {
            p.reclaim.retain(|i| selected.contains(&i.tier));
            for est in &mut p.tiers {
                if !selected.contains(&est.tier) {
                    est.reclaimable_bytes = 0;
                    est.reclaimable_entries = 0;
                }
            }
        }
    }
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

#[derive(serde::Serialize)]
struct DirSummary {
    root: PathBuf,
    freed_bytes: u64,
    notes: Vec<String>,
}

#[derive(Default, serde::Serialize)]
struct SweepTotals {
    freed_bytes: u64,
    residue_paths: u64,
    profiles_skipped_locked: u64,
    dirs: Vec<DirSummary>,
}

/// Sweep every profile of every report, in order. Shared by `gc --apply`
/// and `schedule run`.
fn run_sweep(reports: &[TargetReport], run_id: &str, audit: &mut dyn Write) -> SweepTotals {
    let mut totals = SweepTotals::default();
    for r in reports {
        let mut dir_freed = 0u64;
        let mut notes: Vec<String> = Vec::new();
        for p in &r.profiles {
            match sweep_profile(p, run_id, audit) {
                Ok(SweepOutcome {
                    freed_bytes,
                    residue_paths,
                    skipped_locked: locked,
                    refused,
                    items_skipped,
                    ..
                }) => {
                    dir_freed += freed_bytes;
                    totals.residue_paths += residue_paths;
                    if locked {
                        totals.profiles_skipped_locked += 1;
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
        totals.freed_bytes += dir_freed;
        totals.dirs.push(DirSummary {
            root: r.root.clone(),
            freed_bytes: dir_freed,
            notes,
        });
    }
    totals
}

fn gc(paths: Vec<PathBuf>, apply: bool, tiers: &[TierArg], json: bool) -> ExitCode {
    let mut reports = scan_all(paths);
    filter_tiers(&mut reports, tiers);
    if reports.is_empty() {
        println!("No target directories found under the given paths.");
        return ExitCode::SUCCESS;
    }
    if !apply {
        if json {
            match serde_json::to_string_pretty(&reports) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("error: failed to serialize report: {e}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            print_table(&reports);
            println!("\nDRY RUN — nothing was deleted. Pass --apply to reclaim.");
        }
        return ExitCode::SUCCESS;
    }

    let run_id = run_id("gc");
    let mut audit = match open_audit_log() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: cannot open audit log: {e}");
            return ExitCode::FAILURE;
        }
    };
    let totals = run_sweep(&reports, &run_id, &mut audit);
    let _ = audit.flush();
    if json {
        let summary = serde_json::json!({ "run": run_id, "totals": totals });
        match serde_json::to_string_pretty(&summary) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("error: failed to serialize summary: {e}"),
        }
        return ExitCode::SUCCESS;
    }
    for d in &totals.dirs {
        println!("{:>10} freed  {}", human(d.freed_bytes), d.root.display());
        for n in &d.notes {
            println!("            note: {n}");
        }
    }
    println!(
        "{:>10} freed  TOTAL ({} dirs; {} profiles skipped by live builds; {} residue paths)",
        human(totals.freed_bytes),
        totals.dirs.len(),
        totals.profiles_skipped_locked,
        totals.residue_paths
    );
    ExitCode::SUCCESS
}

/// Adopt every project found under `roots` into the machine registry and
/// report orphaned registry entries (project gone, target dirs reclaimable).
fn scan_cmd(roots: Vec<PathBuf>) -> ExitCode {
    let reports = scan_all(roots);
    let registry = Registry::open(targone_dir().join("registry.jsonl"));
    let mut adopted = 0u64;
    for r in &reports {
        // Conventional `X/target` belongs to project X; a renamed/central
        // build dir is registered as itself.
        let project = if r
            .root
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("target"))
        {
            r.root.parent().unwrap_or(&r.root).to_path_buf()
        } else {
            r.root.clone()
        };
        match registry.record(&project) {
            Ok(()) => adopted += 1,
            Err(e) => eprintln!("warn: could not record {}: {e}", project.display()),
        }
    }
    println!(
        "adopted {adopted} project(s) into {}",
        registry.path().display()
    );
    match registry.entries() {
        Ok(entries) => {
            let orphans: Vec<_> = entries.iter().filter(|e| e.is_orphan()).collect();
            println!(
                "registry now holds {} project(s), {} orphaned",
                entries.len(),
                orphans.len()
            );
            for o in orphans {
                println!(
                    "  orphan: {} (project gone — target dirs eligible for full reclaim)",
                    o.root.display()
                );
            }
        }
        Err(e) => eprintln!("warn: cannot read registry: {e}"),
    }
    print_table(&reports);
    ExitCode::SUCCESS
}

fn schedule_status() -> ExitCode {
    match schedule::status() {
        Ok(s) => println!("scheduler: {s}"),
        Err(e) => println!("scheduler: error: {e}"),
    }
    let last = targone_dir().join("last-run.json");
    match fs::read_to_string(&last) {
        Ok(s) => println!("last scheduled run: {s}"),
        Err(_) => println!("last scheduled run: never"),
    }
    ExitCode::SUCCESS
}

/// One scheduled sweep: config + registry roots, budget-driven selection,
/// silent success, summary persisted for `schedule status`.
fn scheduled_run() -> ExitCode {
    if let Some(reason) = schedule::disabled() {
        println!("targone: disabled ({reason}) — no-op");
        return ExitCode::SUCCESS;
    }
    let cfg = match config::MachineConfig::load(&targone_dir().join("config.toml")) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let budget = match cfg.budget_bytes() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let registry = Registry::open(targone_dir().join("registry.jsonl"));
    let mut roots = cfg.roots.clone();
    if let Ok(entries) = registry.entries() {
        roots.extend(
            entries
                .iter()
                .filter(|e| !e.is_orphan())
                .map(|e| e.root.clone()),
        );
    }
    roots.sort();
    roots.dedup();
    if roots.is_empty() {
        println!(
            "targone: nothing to do — add roots to {} or run `cargo targone scan <dirs>`",
            targone_dir().join("config.toml").display()
        );
        return ExitCode::SUCCESS;
    }

    let reports = scan_all(roots);
    let pairs: Vec<(u64, u64)> = reports
        .iter()
        .map(|r| (r.total_bytes(), r.reclaimable_bytes()))
        .collect();
    let (selected, plan) = select_for_budget(&pairs, budget);
    let chosen: Vec<TargetReport> = {
        let mut idx: Vec<bool> = vec![false; reports.len()];
        for i in &selected {
            idx[*i] = true;
        }
        reports
            .into_iter()
            .zip(idx)
            .filter_map(|(r, keep)| keep.then_some(r))
            .collect()
    };

    let run_id = run_id("sched");
    let mut audit = match open_audit_log() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: cannot open audit log: {e}");
            return ExitCode::FAILURE;
        }
    };
    let totals = run_sweep(&chosen, &run_id, &mut audit);
    let _ = audit.flush();

    let summary = serde_json::json!({
        "run": run_id,
        "ts": now_secs(),
        "budget": budget,
        "plan": plan,
        "totals": totals,
    });
    if let Ok(s) = serde_json::to_string(&summary) {
        let _ = fs::write(targone_dir().join("last-run.json"), s);
    }
    println!(
        "targone: freed {} across {} dir(s) ({} skipped by live builds, {} residue){}",
        human(totals.freed_bytes),
        totals.dirs.len(),
        totals.profiles_skipped_locked,
        totals.residue_paths,
        if plan.insufficient {
            " — budget unreachable even sweeping everything"
        } else {
            ""
        }
    );
    ExitCode::SUCCESS
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn run_id(prefix: &str) -> String {
    format!("{prefix}-{}", now_secs())
}

/// Append-only audit log at `$CARGO_HOME/targone/audit.jsonl`.
fn open_audit_log() -> std::io::Result<impl Write> {
    let dir = targone_dir();
    fs::create_dir_all(&dir)?;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("audit.jsonl"))
}

fn targone_dir() -> PathBuf {
    cargo_home().join("targone")
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
