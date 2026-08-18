//! Phase-2 exit gate 5.1: a sweep loop must never break a concurrent build.
//!
//! 100 iterations of touch + `cargo build` against an in-process
//! scan+sweep loop on the same profile dir. Zero build failures required.
//! Ignored by default (needs `cargo` on PATH and ~1–3 min); CI runs it with
//! `cargo test -p targone-core --test concurrency_gate -- --ignored`.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use targone_core::{layout::probe_profile, scan::scan_profile, sweep::sweep_profile};

fn write(p: &Path, content: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

#[test]
#[ignore = "spawns 100 real cargo builds; run explicitly in CI"]
fn hundred_builds_survive_a_continuous_sweep_loop() {
    let t = tempfile::tempdir().unwrap();
    let proj = t.path().join("probe");
    write(
        &proj.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&proj.join("src/main.rs"), "fn main() { println!(\"0\"); }");
    // Dep-free on purpose: offline-safe in CI; the gate exercises LOCKING,
    // not classification breadth.
    let ok = Command::new("cargo")
        .arg("build")
        .current_dir(&proj)
        .status()
        .expect("cargo on PATH")
        .success();
    assert!(ok, "initial build failed");
    let profile = proj.join("target/debug");

    let stop = AtomicBool::new(false);
    let sweeps_attempted = std::sync::atomic::AtomicU64::new(0);
    std::thread::scope(|s| {
        s.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                let layout = probe_profile(&profile);
                let report = scan_profile(&profile, layout);
                let mut audit = Vec::new();
                // Errors here would be real bugs; lock-skips are expected.
                sweep_profile(&report, "gate", &mut audit).expect("sweep must not error");
                sweeps_attempted.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let mut failures = 0u32;
        for i in 0..100 {
            // Alternate touched content so every build has real work.
            write(
                &proj.join("src/main.rs"),
                &format!("fn main() {{ println!(\"{i}\"); }}"),
            );
            let now = SystemTime::now();
            let _ = now; // mtime update happens via the write above
            let out = Command::new("cargo")
                .args(["build"])
                .current_dir(&proj)
                .output()
                .expect("cargo runs");
            if !out.status.success() {
                failures += 1;
                eprintln!(
                    "build {i} FAILED:\n{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }
        stop.store(true, Ordering::Relaxed);
        assert_eq!(failures, 0, "concurrent sweeps broke builds");
    });
    assert!(
        sweeps_attempted.load(Ordering::Relaxed) > 10,
        "sweep loop barely ran — gate not meaningful"
    );

    // Post-storm: the tree must still be buildable and converge to fresh.
    let out = Command::new("cargo")
        .arg("build")
        .current_dir(&proj)
        .output()
        .unwrap();
    assert!(out.status.success(), "post-storm build failed");
}
