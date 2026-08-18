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
        /// OPT-IN tier 5: also reclaim every .pdb under deps/ and build/
        /// (debug symbols; worst case is a re-link to regenerate them).
        #[arg(long)]
        pdbs: bool,
        /// OPT-IN tier 6: full pool wipe of profiles whose newest compile is
        /// older than DAYS (age relative to the dir's own last build, F-043).
        #[arg(long, value_name = "DAYS")]
        dormant: Option<u64>,
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
            pdbs,
            dormant,
            json,
        } => gc(paths, apply, &tier, pdbs, dormant, json),
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
    let mut reports = scan_all(paths);
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
    // Quantified opt-in advice (analysis F-042/F-070): measured on the dirs
    // just scanned, never applied automatically.
    let mut pdb_bytes = 0u64;
    for r in &mut reports {
        for p in &mut r.profiles {
            targone_core::append_pdb_items(p);
            if let Some(t) = p.tiers.iter().find(|t| t.tier == targone_core::Tier::Pdb) {
                pdb_bytes += t.reclaimable_bytes;
            }
        }
    }
    if pdb_bytes > 64 * 1024 * 1024 {
        println!(
            "advice: `gc --pdbs` would reclaim a further {} of debug symbols (worst case: a re-link regenerates them)",
            human(pdb_bytes)
        );
    }
    println!("advice: `gc --dormant <days>` wipes profiles idle longer than <days>; set `pdbs`/`dormant_days` in {} for scheduled runs", targone_dir().join("config.toml").display());
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

/// Apply the opt-in tiers to a scanned report set. Dormancy first — its
/// whole-pool wipe supersedes finer plans — then PDBs (which skip anything
/// already planned).
fn apply_opt_ins(reports: &mut [TargetReport], pdbs: bool, dormant_days: Option<u64>) {
    if !pdbs && dormant_days.is_none() {
        return;
    }
    let cutoff = dormant_days
        .map(|d| SystemTime::now() - std::time::Duration::from_secs(d.saturating_mul(86_400)));
    for r in reports.iter_mut() {
        for p in &mut r.profiles {
            if let Some(cutoff) = cutoff {
                let _ = targone_core::append_dormant_item(p, cutoff);
            }
            if pdbs {
                targone_core::append_pdb_items(p);
            }
        }
    }
}

fn gc(
    paths: Vec<PathBuf>,
    apply: bool,
    tiers: &[TierArg],
    pdbs: bool,
    dormant: Option<u64>,
    json: bool,
) -> ExitCode {
    let mut reports = scan_all(paths);
    filter_tiers(&mut reports, tiers);
    apply_opt_ins(&mut reports, pdbs, dormant);
    reports.sort_by_key(|r| std::cmp::Reverse(r.reclaimable_bytes()));
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
        // Serialize fallibly: paths with invalid UTF-8 must degrade to an
        // error line, never a panic (the json! macro unwraps internally).
        #[derive(serde::Serialize)]
        struct ApplySummary<'a> {
            run: &'a str,
            totals: &'a SweepTotals,
        }
        match serde_json::to_string_pretty(&ApplySummary {
            run: &run_id,
            totals: &totals,
        }) {
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
    // An unreadable registry degrades to config roots only.
    roots.extend(
        registry
            .entries()
            .into_iter()
            .flatten()
            .filter(|e| !e.is_orphan())
            .map(|e| e.root),
    );
    roots.sort();
    roots.dedup();
    if roots.is_empty() {
        println!(
            "targone: nothing to do — add roots to {} or run `cargo targone scan <dirs>`",
            targone_dir().join("config.toml").display()
        );
        return ExitCode::SUCCESS;
    }

    let mut reports = scan_all(roots);
    apply_opt_ins(&mut reports, cfg.pdbs, cfg.dormant_days);
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

    #[derive(serde::Serialize)]
    struct RunSummary<'a> {
        run: &'a str,
        ts: u64,
        budget: Option<u64>,
        plan: targone_core::BudgetPlan,
        totals: &'a SweepTotals,
    }
    // Fallible on purpose: invalid-UTF-8 paths must never panic a run.
    if let Ok(s) = serde_json::to_string(&RunSummary {
        run: &run_id,
        ts: now_secs(),
        budget,
        plan,
        totals: &totals,
    }) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use targone_core::{ProfileLayout, ProfileReport};

    fn bare_profile(path: PathBuf, layout: ProfileLayout) -> ProfileReport {
        ProfileReport {
            path,
            layout,
            pools: Default::default(),
            tiers: Vec::new(),
            reclaim: Vec::new(),
            unparsed_kept: 0,
        }
    }

    fn all_notes(totals: &SweepTotals) -> String {
        totals
            .dirs
            .iter()
            .flat_map(|d| d.notes.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn run_sweep_notes_errors() {
        // A profile whose path is a FILE makes the lock open fail — a
        // genuine sweep error, reported as a note, never a crash.
        let t = tempfile::tempdir().unwrap();
        let file_as_profile = t.path().join("iamafile");
        fs::write(&file_as_profile, b"x").unwrap();
        let errored = TargetReport {
            root: t.path().to_path_buf(),
            profiles: vec![bare_profile(file_as_profile, ProfileLayout::LegacyBuild)],
            root_pools: Default::default(),
        };
        let mut audit = Vec::new();
        let totals = run_sweep(&[errored], "test", &mut audit);
        assert!(
            all_notes(&totals).contains("error:"),
            "{}",
            all_notes(&totals)
        );
        assert_eq!(totals.freed_bytes, 0);
    }

    #[cfg(windows)]
    #[test]
    fn run_sweep_notes_network_refusals() {
        // UNC semantics are Windows-only: on Linux the same path is just a
        // relative dir that fails to open (covered by the errors test).
        let refused = TargetReport {
            root: PathBuf::from(r"\\no-such-host\share\target"),
            profiles: vec![bare_profile(
                PathBuf::from(r"\\no-such-host\share\target\debug"),
                ProfileLayout::LegacyBuild,
            )],
            root_pools: Default::default(),
        };
        let mut audit = Vec::new();
        let totals = run_sweep(&[refused], "test", &mut audit);
        assert!(
            all_notes(&totals).contains("refused (network filesystem)"),
            "{}",
            all_notes(&totals)
        );
        assert_eq!(totals.freed_bytes, 0);
    }
}
