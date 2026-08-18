//! The sweep executor: applies a profile's [`ReclaimItem`]s under Cargo's
//! lock protocol (F-061). This is the only module in the crate that deletes.
//!
//! Invariants:
//! - network filesystems are refused (Cargo skips locking there — no
//!   protocol to join);
//! - the layout is re-validated under the held lock (closes the
//!   check-then-delete window, F-054);
//! - `try_lock` and skip — never wait, never proceed unlocked;
//! - `delete_first` before `delete_then` (fingerprint before artifacts);
//! - Windows deletion failures are retried with backoff and tolerated as
//!   residue, never escalated to a failed run (F-053).

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::layout::probe_profile;
use crate::lock::{session_lock_free, try_lock_profile};
use crate::scan::ProfileReport;

#[derive(Debug, Default, Serialize)]
pub struct SweepOutcome {
    pub freed_bytes: u64,
    pub freed_entries: u64,
    pub items_swept: u64,
    pub items_skipped: u64,
    /// Paths that survived all retries (open handles, AV) — logged, not fatal.
    pub residue_paths: u64,
    /// A build held the profile locks; nothing was touched.
    pub skipped_locked: bool,
    /// The profile was refused outright (network FS, layout drift).
    pub refused: Option<String>,
}

/// Sweep one scanned profile. `audit` receives one JSON line per item.
pub fn sweep_profile(
    report: &ProfileReport,
    run_id: &str,
    audit: &mut dyn Write,
) -> io::Result<SweepOutcome> {
    let mut outcome = SweepOutcome::default();

    if crate::fsinfo::is_network_path(&report.path) {
        outcome.refused = Some("network filesystem".to_string());
        return Ok(outcome);
    }
    let Some(_guard) = try_lock_profile(&report.path)? else {
        outcome.skipped_locked = true;
        return Ok(outcome);
    };
    // Re-validate under the lock: the world may have changed since the scan.
    if probe_profile(&report.path) != report.layout {
        outcome.refused = Some("layout changed since scan".to_string());
        return Ok(outcome);
    }

    for item in &report.reclaim {
        // rustc session locks: all must be free, else skip the whole item
        // (spike 0.4 refusal rule 3).
        if !item.session_locks.iter().all(|l| session_lock_free(l)) {
            outcome.items_skipped += 1;
            audit_line(audit, run_id, item, "skipped-session-lock", &[]);
            continue;
        }
        let mut residue: Vec<String> = Vec::new();
        for dir in &item.delete_first {
            if let Err(e) = delete_dir_tolerant(dir) {
                residue.push(format!("{}: {e}", dir.display()));
            }
        }
        for f in &item.delete_then {
            if let Err(e) = delete_file_tolerant(f) {
                residue.push(format!("{}: {e}", f.display()));
            }
        }
        if residue.is_empty() {
            outcome.items_swept += 1;
            outcome.freed_bytes += item.bytes;
            outcome.freed_entries += item.entries;
            audit_line(audit, run_id, item, "swept", &[]);
        } else {
            outcome.residue_paths += residue.len() as u64;
            outcome.items_swept += 1;
            audit_line(audit, run_id, item, "swept-with-residue", &residue);
        }
    }
    Ok(outcome)
}

#[derive(Serialize)]
struct AuditRecord<'a> {
    run: &'a str,
    ts: u64,
    status: &'a str,
    tier: crate::scan::Tier,
    bytes: u64,
    entries: u64,
    delete_first: &'a [std::path::PathBuf],
    delete_then: &'a [std::path::PathBuf],
    residue: &'a [String],
}

fn audit_line(
    audit: &mut dyn Write,
    run_id: &str,
    item: &crate::scan::ReclaimItem,
    status: &str,
    residue: &[String],
) {
    let record = AuditRecord {
        run: run_id,
        ts: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        status,
        tier: item.tier,
        bytes: item.bytes,
        entries: item.entries,
        delete_first: &item.delete_first,
        delete_then: &item.delete_then,
        residue,
    };
    if let Ok(line) = serde_json::to_string(&record) {
        let _ = writeln!(audit, "{line}");
    }
}

/// Windows-transient errors worth retrying: sharing violations (32) and
/// access-denied from AV scanners or delete-pending handles (5). Measured on
/// the reference machine: Windows Defender scans never-before-seen
/// executables ON DELETE, holding handles for hundreds of ms to seconds —
/// the dominant residue source for `build/` dirs. Residue that outlives the
/// retry window is re-collected by the next run (the fingerprint-less
/// artifact rule in `scan`), so the window is a latency/politeness knob,
/// not a correctness one.
fn retryable(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(5) | Some(32))
}

/// A path that is already gone is a success, not an error.
fn ignore_missing(r: io::Result<()>) -> io::Result<()> {
    match r {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn with_retry(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    // 50 → 150 → 450 → 1350 ms between attempts: ~2 s total, enough for
    // most Defender scan-on-delete holds without stalling a large sweep.
    let mut delay = Duration::from_millis(50);
    for _ in 0..4 {
        match ignore_missing(op()) {
            Ok(()) => return Ok(()),
            Err(e) if retryable(&e) => {
                std::thread::sleep(delay);
                delay *= 3;
            }
            Err(e) => return Err(e),
        }
    }
    // Final attempt: whatever error remains is the caller's residue.
    ignore_missing(op())
}

fn delete_dir_tolerant(dir: &Path) -> io::Result<()> {
    // Refuse to traverse a symlink pretending to be a directory.
    if fs::symlink_metadata(dir)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(io::Error::other("refusing to delete through a symlink"));
    }
    with_retry(|| match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        // The remove_dir_all crate uses a different Windows strategy
        // (POSIX-semantics delete). Measured on the reference machine: std
        // succeeds on dirs holding executables where the crate returns
        // os error 5 — so std leads and the crate is the fallback for
        // delete-pending/long-path cases std cannot handle.
        Err(_) => remove_dir_all::remove_dir_all(dir),
    })
}

fn delete_file_tolerant(file: &Path) -> io::Result<()> {
    with_retry(|| fs::remove_file(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::ProfileLayout;
    use crate::scan::scan_profile;
    use std::fs::File;
    use std::path::Path;
    use std::thread::sleep;
    use std::time::Duration as StdDuration;

    fn write(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0u8; bytes]).unwrap();
    }

    fn lib_unit(profile: &Path, pkg: &str, hash: &str, rlib_bytes: usize) {
        let fp = profile.join(".fingerprint").join(format!("{pkg}-{hash}"));
        write(&fp.join(format!("lib-{pkg}")), 16);
        write(&fp.join(format!("lib-{pkg}.json")), 32);
        write(&fp.join("invoked.timestamp"), 8);
        write(
            &profile.join("deps").join(format!("lib{pkg}-{hash}.rlib")),
            rlib_bytes,
        );
    }

    #[test]
    fn sweep_removes_superseded_keeps_newest() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        sleep(StdDuration::from_millis(60));
        lib_unit(&profile, "serde", "bbbbbbbbbbbbbbbb", 2000);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let mut audit = Vec::new();
        let outcome = sweep_profile(&report, "test-run", &mut audit).unwrap();
        assert_eq!(outcome.items_swept, 1);
        assert_eq!(outcome.freed_bytes, 1056);
        assert_eq!(outcome.residue_paths, 0);
        assert!(!profile.join(".fingerprint/serde-aaaaaaaaaaaaaaaa").exists());
        assert!(!profile.join("deps/libserde-aaaaaaaaaaaaaaaa.rlib").exists());
        assert!(profile.join(".fingerprint/serde-bbbbbbbbbbbbbbbb").is_dir());
        assert!(profile
            .join("deps/libserde-bbbbbbbbbbbbbbbb.rlib")
            .is_file());
        let log = String::from_utf8(audit).unwrap();
        assert!(log.contains("\"status\":\"swept\""));
    }

    #[test]
    fn sweep_skips_when_build_lock_held() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        sleep(StdDuration::from_millis(60));
        lib_unit(&profile, "serde", "bbbbbbbbbbbbbbbb", 2000);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        // Simulate a live cargo build holding the exclusive build lock.
        let cargo = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(profile.join(".cargo-build-lock"))
            .unwrap();
        cargo.try_lock().unwrap();
        let mut audit = Vec::new();
        let outcome = sweep_profile(&report, "test-run", &mut audit).unwrap();
        assert!(outcome.skipped_locked);
        assert_eq!(outcome.items_swept, 0);
        // Nothing was deleted.
        assert!(profile
            .join("deps/libserde-aaaaaaaaaaaaaaaa.rlib")
            .is_file());
    }

    #[test]
    fn sweep_skips_item_when_session_lock_held() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::create_dir_all(profile.join("deps")).unwrap();
        let inc = profile.join("incremental");
        write(&inc.join("mycrate-aaaa/s-x-y-z/dep-graph.bin"), 300);
        write(&inc.join("mycrate-aaaa/s-x-y.lock"), 0);
        sleep(StdDuration::from_millis(60));
        write(&inc.join("mycrate-bbbb/s-x-w-z/dep-graph.bin"), 400);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        // A rustc session holds the old dir's lock.
        let rustc = File::options()
            .read(true)
            .write(true)
            .open(inc.join("mycrate-aaaa/s-x-y.lock"))
            .unwrap();
        rustc.try_lock().unwrap();
        let mut audit = Vec::new();
        let outcome = sweep_profile(&report, "test-run", &mut audit).unwrap();
        assert_eq!(outcome.items_skipped, 1);
        assert!(inc.join("mycrate-aaaa").is_dir());
        let log = String::from_utf8(audit).unwrap();
        assert!(log.contains("skipped-session-lock"));
    }

    #[test]
    fn sweep_refuses_network_paths() {
        let report = crate::scan::ProfileReport {
            path: std::path::PathBuf::from(r"\\definitely-not-a-real-host\share\debug"),
            layout: ProfileLayout::LegacyBuild,
            pools: Default::default(),
            tiers: Vec::new(),
            reclaim: Vec::new(),
            unparsed_kept: 0,
        };
        let mut audit = Vec::new();
        let outcome = sweep_profile(&report, "test-run", &mut audit).unwrap();
        assert_eq!(outcome.refused.as_deref(), Some("network filesystem"));
    }

    #[test]
    fn missing_paths_are_tolerated_not_errors() {
        let t = tempfile::tempdir().unwrap();
        assert!(delete_dir_tolerant(&t.path().join("never-was")).is_ok());
        assert!(delete_file_tolerant(&t.path().join("never-was.rlib")).is_ok());
    }

    #[test]
    fn invalid_paths_error_without_retry() {
        // A filename Windows/Unix cannot even address: non-retryable Err arm.
        let bad = Path::new("con\u{0}bad/\u{0}x");
        assert!(delete_file_tolerant(bad).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn held_handle_without_share_delete_becomes_residue() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        sleep(StdDuration::from_millis(60));
        lib_unit(&profile, "serde", "bbbbbbbbbbbbbbbb", 2000);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        // Hold the superseded rlib AND a file inside the superseded
        // fingerprint dir with share_mode(0): both the delete_then file path
        // and the delete_first dir path (std then crate fallback) fail
        // through every retry — the residue path, tolerated, never fatal.
        let _hold_file = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(profile.join("deps/libserde-aaaaaaaaaaaaaaaa.rlib"))
            .unwrap();
        let _hold_in_dir = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(profile.join(".fingerprint/serde-aaaaaaaaaaaaaaaa/lib-serde"))
            .unwrap();
        let mut audit = Vec::new();
        let outcome = sweep_profile(&report, "test-run", &mut audit).unwrap();
        assert!(outcome.residue_paths >= 2);
        let log = String::from_utf8(audit).unwrap();
        assert!(log.contains("swept-with-residue"), "{log}");
    }

    #[cfg(windows)]
    #[test]
    fn junction_dirs_are_refused() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        fs::create_dir_all(&real).unwrap();
        let link = t.path().join("link");
        let ok = std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                link.to_str().unwrap(),
                real.to_str().unwrap(),
            ])
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "junction creation failed");
        let err = delete_dir_tolerant(&link).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(real.exists());
    }

    #[test]
    fn sweep_refuses_on_layout_drift() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        sleep(StdDuration::from_millis(60));
        lib_unit(&profile, "serde", "bbbbbbbbbbbbbbbb", 2000);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        // The world changes between scan and sweep.
        fs::remove_dir_all(profile.join("deps")).unwrap();
        let mut audit = Vec::new();
        let outcome = sweep_profile(&report, "test-run", &mut audit).unwrap();
        assert_eq!(
            outcome.refused.as_deref(),
            Some("layout changed since scan")
        );
        assert!(profile.join(".fingerprint/serde-aaaaaaaaaaaaaaaa").is_dir());
    }
}
