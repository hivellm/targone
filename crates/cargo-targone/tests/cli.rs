//! End-to-end CLI tests: spawn the real binary (coverage flows from
//! subprocesses under cargo-llvm-cov). Every run is isolated behind a temp
//! `CARGO_HOME`; scheduler tests use a disposable task name via
//! `TARGONE_TASK_NAME` so the real `Targone` task is never touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_cargo-targone")
}

fn run_in(home: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .env("CARGO_HOME", home)
        .env_remove("CI")
        .env_remove("TARGONE_DISABLE")
        .env_remove("TARGONE_TASK_NAME")
        .output()
        .expect("binary runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn write(p: &Path, bytes: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![0u8; bytes]).unwrap();
}

/// A conventional project with a legacy target dir holding one superseded
/// lib generation, an incremental duplicate, and a stray pdb.
fn fixture_project(root: &Path) -> PathBuf {
    let proj = root.join("proj");
    write(&proj.join("src/main.rs"), 10);
    let profile = proj.join("target/debug");
    // Both generations carry the same artifact-extension set ({pdb, rlib}) —
    // same identity class, so the older one is genuinely superseded.
    for (hash, size) in [("aaaaaaaaaaaaaaaa", 1000), ("bbbbbbbbbbbbbbbb", 2000)] {
        let fp = profile.join(".fingerprint").join(format!("serde-{hash}"));
        write(&fp.join("lib-serde"), 16);
        write(&fp.join("lib-serde.json"), 32);
        write(&fp.join("invoked.timestamp"), 8);
        write(&profile.join(format!("deps/libserde-{hash}.rlib")), size);
        write(&profile.join(format!("deps/serde-{hash}.pdb")), 700);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    write(&profile.join("incremental/mycrate-aa/s-x-y-z/o.bin"), 300);
    std::thread::sleep(std::time::Duration::from_millis(30));
    write(&profile.join("incremental/mycrate-bb/s-x-w-z/o.bin"), 400);
    proj
}

#[test]
fn report_human_json_and_empty() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    fixture_project(t.path());
    let o = run_in(&home, &["report", t.path().to_str().unwrap()]);
    assert!(o.status.success());
    let s = stdout(&o);
    assert!(s.contains("TOTAL (1 dirs)"), "{s}");
    assert!(s.contains("advice:"), "{s}");

    let o = run_in(&home, &["report", "--json", t.path().to_str().unwrap()]);
    assert!(o.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 1);

    let empty = t.path().join("nothing");
    fs::create_dir_all(&empty).unwrap();
    let o = run_in(&home, &["report", empty.to_str().unwrap()]);
    assert!(stdout(&o).contains("No target directories"));
}

#[test]
fn cargo_style_invocation_drops_duplicated_subcommand() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let empty = t.path().join("nothing");
    fs::create_dir_all(&empty).unwrap();
    // `cargo targone …` invokes us as `cargo-targone targone …`.
    let o = run_in(&home, &["targone", "report", empty.to_str().unwrap()]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("No target directories"));
}

#[test]
fn gc_dry_run_apply_and_json() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let proj = fixture_project(t.path());
    let root = t.path().to_str().unwrap().to_string();

    let o = run_in(&home, &["gc", &root]);
    assert!(stdout(&o).contains("DRY RUN"));
    assert!(proj
        .join("target/debug/deps/libserde-aaaaaaaaaaaaaaaa.rlib")
        .exists());

    let o = run_in(&home, &["gc", "--json", &root]);
    assert!(serde_json::from_str::<serde_json::Value>(&stdout(&o)).is_ok());

    let o = run_in(&home, &["gc", "--apply", "--json", &root]);
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(v["totals"]["freed_bytes"].as_u64().unwrap() > 0);
    assert!(!proj
        .join("target/debug/deps/libserde-aaaaaaaaaaaaaaaa.rlib")
        .exists());
    assert!(proj
        .join("target/debug/deps/libserde-bbbbbbbbbbbbbbbb.rlib")
        .exists());
    assert!(home.join("targone/audit.jsonl").is_file());

    // Converged: human apply output prints totals.
    let o = run_in(&home, &["gc", "--apply", &root]);
    assert!(stdout(&o).contains("TOTAL"));

    // Empty root.
    let empty = t.path().join("nothing");
    fs::create_dir_all(&empty).unwrap();
    let o = run_in(&home, &["gc", empty.to_str().unwrap()]);
    assert!(stdout(&o).contains("No target directories"));
}

#[test]
fn gc_tier_filter_pdbs_and_dormant() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let proj = fixture_project(t.path());
    let root = t.path().to_str().unwrap().to_string();

    // Only the incremental tier: the superseded rlib must survive.
    let o = run_in(&home, &["gc", "--apply", "--tier", "incremental", &root]);
    assert!(o.status.success());
    assert!(proj
        .join("target/debug/deps/libserde-aaaaaaaaaaaaaaaa.rlib")
        .exists());
    assert!(!proj.join("target/debug/incremental/mycrate-aa").exists());

    // PDB opt-in reclaims the live pdb.
    let o = run_in(&home, &["gc", "--apply", "--pdbs", &root]);
    assert!(o.status.success());
    assert!(!proj
        .join("target/debug/deps/serde-bbbbbbbbbbbbbbbb.pdb")
        .exists());

    // Dormant with a huge threshold: nothing is that old.
    let o = run_in(&home, &["gc", "--apply", "--dormant", "3650", &root]);
    assert!(o.status.success());
    assert!(proj
        .join("target/debug/deps/libserde-bbbbbbbbbbbbbbbb.rlib")
        .exists());

    // Dormant 0 days: everything compiled before "now - 0" is wiped.
    let o = run_in(&home, &["gc", "--apply", "--dormant", "0", &root]);
    assert!(o.status.success());
    assert!(!proj.join("target/debug/deps").exists());
    assert!(proj.join("target/debug").exists());
}

#[test]
fn scan_adopts_and_reports_orphans() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    fixture_project(t.path());
    // Pre-seed an orphan registry entry.
    fs::create_dir_all(home.join("targone")).unwrap();
    fs::write(
        home.join("targone/registry.jsonl"),
        format!(
            "{{\"v\":1,\"root\":\"{}\",\"ts\":5}}\n",
            t.path()
                .join("gone")
                .display()
                .to_string()
                .replace('\\', "\\\\")
        ),
    )
    .unwrap();
    let o = run_in(&home, &["scan", t.path().to_str().unwrap()]);
    assert!(o.status.success());
    let s = stdout(&o);
    assert!(s.contains("adopted 1 project(s)"), "{s}");
    assert!(s.contains("1 orphaned"), "{s}");
    assert!(s.contains("orphan:"), "{s}");
}

#[test]
fn schedule_run_paths() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");

    // Disabled switch.
    let o = Command::new(bin())
        .args(["schedule", "run"])
        .env("CARGO_HOME", &home)
        .env("TARGONE_DISABLE", "1")
        .output()
        .unwrap();
    assert!(stdout(&o).contains("disabled"));

    // No roots configured.
    let o = run_in(&home, &["schedule", "run"]);
    assert!(stdout(&o).contains("nothing to do"), "{}", stdout(&o));

    // Roots + budget: full scheduled sweep with last-run summary.
    fixture_project(t.path());
    fs::create_dir_all(home.join("targone")).unwrap();
    fs::write(
        home.join("targone/config.toml"),
        format!(
            "budget = \"1KiB\"\nroots = [\"{}\"]\npdbs = true\n",
            t.path().display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let o = run_in(&home, &["schedule", "run"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("targone: freed"), "{}", stdout(&o));
    assert!(home.join("targone/last-run.json").is_file());

    // status shows the recorded run (scheduler state text is platform-run).
    let o = run_in(&home, &["schedule", "status"]);
    assert!(stdout(&o).contains("last scheduled run:"));

    // Invalid budget → failure.
    fs::write(home.join("targone/config.toml"), "budget = \"banana\"\n").unwrap();
    let o = run_in(&home, &["schedule", "run"]);
    assert!(!o.status.success());

    // Invalid config → failure.
    fs::write(home.join("targone/config.toml"), "not toml at all [[[").unwrap();
    let o = run_in(&home, &["schedule", "run"]);
    assert!(!o.status.success());
}

#[cfg(windows)]
#[test]
fn schedule_install_status_uninstall_with_disposable_task() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let task = format!("TargoneCovTest{}", std::process::id());
    let run = |args: &[&str]| {
        Command::new(bin())
            .args(args)
            .env("CARGO_HOME", &home)
            .env("TARGONE_TASK_NAME", &task)
            .env_remove("CI")
            .env_remove("TARGONE_DISABLE")
            .output()
            .unwrap()
    };
    // Not installed yet.
    let o = run(&["schedule", "status"]);
    assert!(stdout(&o).contains("not installed"), "{}", stdout(&o));
    // Install (idempotent), status, uninstall — always clean up.
    let o = run(&["schedule", "install"]);
    let installed = o.status.success();
    let status_out = stdout(&run(&["schedule", "status"]));
    let o = run(&["schedule", "uninstall"]);
    assert!(o.status.success(), "cleanup must succeed");
    assert!(stdout(&o).contains("removed"));
    assert!(installed, "install failed");
    assert!(status_out.contains("state="), "{status_out}");
}

#[test]
fn report_notes_advice_and_default_path() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let proj = fixture_project(t.path());
    // An unparsed entry (stray file in incremental/) and a >64 MiB pdb.
    write(&proj.join("target/debug/incremental/stray.txt"), 10);
    write(
        &proj.join("target/debug/deps/big.pdb"),
        65 * 1024 * 1024 + 1,
    );
    let o = run_in(&home, &["report", t.path().to_str().unwrap()]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("fail-open"));
    assert!(
        stdout(&o).contains("gc --pdbs` would reclaim"),
        "{}",
        stdout(&o)
    );

    // Default path: current directory.
    let empty = t.path().join("elsewhere");
    fs::create_dir_all(&empty).unwrap();
    let o = Command::new(bin())
        .arg("report")
        .current_dir(&empty)
        .env("CARGO_HOME", &home)
        .output()
        .unwrap();
    assert!(stdout(&o).contains("No target directories"));
}

#[test]
fn gc_remaining_tier_args_parse() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    fixture_project(t.path());
    let o = run_in(
        &home,
        &[
            "gc",
            "--tier",
            "units",
            "--tier",
            "build-scripts",
            "--tier",
            "orphan-fingerprints",
            t.path().to_str().unwrap(),
        ],
    );
    assert!(o.status.success());
}

#[test]
fn gc_apply_notes_for_locked_profile_and_session_locks() {
    use std::fs::File;
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let proj = fixture_project(t.path());
    let root = t.path().to_str().unwrap().to_string();
    let profile = proj.join("target/debug");

    // Hold the build lock: human output prints the skip note.
    let lock_path = profile.join(".cargo-build-lock");
    File::create(&lock_path).unwrap();
    let holder = File::options()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    holder.try_lock().unwrap();
    let o = run_in(&home, &["gc", "--apply", &root]);
    assert!(
        stdout(&o).contains("build in progress, skipped"),
        "{}",
        stdout(&o)
    );
    holder.unlock().unwrap();
    drop(holder);

    // Hold a rustc session lock: the item-skip note path. Creating the lock
    // bumps mycrate-aa's dir mtime, so age mycrate-bb afterwards to keep aa
    // as the superseded (locked) generation.
    let session = profile.join("incremental/mycrate-aa/s-x-y.lock");
    write(&session, 0);
    std::thread::sleep(std::time::Duration::from_millis(40));
    // A NEW session dir: a direct child, so mycrate-bb's own mtime advances
    // (grandchild writes would not touch it).
    write(&profile.join("incremental/mycrate-bb/s-x2-w-z/o.bin"), 10);
    let session_holder = File::options()
        .read(true)
        .write(true)
        .open(&session)
        .unwrap();
    session_holder.try_lock().unwrap();
    let o = run_in(&home, &["gc", "--apply", &root]);
    assert!(
        stdout(&o).contains("items skipped (session locks)"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn audit_log_failure_is_fatal_for_apply_and_scheduled() {
    let t = tempfile::tempdir().unwrap();
    fixture_project(t.path());
    // CARGO_HOME pointing at a FILE: the targone dir cannot be created.
    let bogus_home = t.path().join("not-a-dir");
    fs::write(&bogus_home, b"file").unwrap();
    let o = run_in(&bogus_home, &["gc", "--apply", t.path().to_str().unwrap()]);
    assert!(!o.status.success());

    // Scheduled run: valid config but audit.jsonl exists as a DIRECTORY.
    let home = t.path().join("home2");
    fs::create_dir_all(home.join("targone/audit.jsonl")).unwrap();
    fs::write(
        home.join("targone/config.toml"),
        format!(
            "roots = [\"{}\"]\n",
            t.path().display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let o = run_in(&home, &["schedule", "run"]);
    assert!(!o.status.success());
}

#[test]
fn scan_renamed_build_dir_and_registry_failures() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    // A central build dir (marker, not named `target`) registers as itself.
    let bdir = t.path().join("central-build");
    write(&bdir.join(".rustc_info.json"), 2);
    fs::create_dir_all(bdir.join("debug/.fingerprint")).unwrap();
    fs::create_dir_all(bdir.join("debug/deps")).unwrap();
    let o = run_in(&home, &["scan", t.path().to_str().unwrap()]);
    assert!(stdout(&o).contains("adopted 1 project(s)"));

    // registry.jsonl as a DIRECTORY: record warns, entries errors.
    let broken_home = t.path().join("broken-home");
    fs::create_dir_all(broken_home.join("targone/registry.jsonl")).unwrap();
    let o = run_in(&broken_home, &["scan", t.path().to_str().unwrap()]);
    assert!(o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("could not record"), "{err}");
    assert!(err.contains("cannot read registry"), "{err}");
}

#[test]
fn scheduled_run_without_budget_and_under_ci() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    fixture_project(t.path());
    fs::create_dir_all(home.join("targone")).unwrap();
    fs::write(
        home.join("targone/config.toml"),
        format!(
            "roots = [\"{}\"]\n",
            t.path().display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    // No budget: sweeps everything reclaimable, no "unreachable" suffix.
    let o = run_in(&home, &["schedule", "run"]);
    assert!(o.status.success());
    let s = stdout(&o);
    assert!(
        s.contains("targone: freed") && !s.contains("unreachable"),
        "{s}"
    );

    // CI environments are a hard no-op.
    let o = Command::new(bin())
        .args(["schedule", "run"])
        .env("CARGO_HOME", &home)
        .env("CI", "true")
        .env_remove("TARGONE_DISABLE")
        .output()
        .unwrap();
    assert!(stdout(&o).contains("disabled (CI environment)"));
}

#[test]
fn cargo_home_fallback_chain() {
    let t = tempfile::tempdir().unwrap();
    let run_env = |userprofile: Option<&Path>, home: Option<&Path>| {
        let mut c = Command::new(bin());
        c.args(["schedule", "status"])
            .env_remove("CARGO_HOME")
            .env_remove("USERPROFILE")
            .env_remove("HOME")
            .env("TARGONE_TASK_NAME", "TargoneCovNoSuchTask");
        if let Some(u) = userprofile {
            c.env("USERPROFILE", u);
        }
        if let Some(h) = home {
            c.env("HOME", h);
        }
        c.output().unwrap()
    };
    // USERPROFILE, then HOME, then ".".
    for (u, h) in [(Some(t.path()), None), (None, Some(t.path())), (None, None)] {
        let o = run_env(u, h);
        assert!(
            stdout(&o).contains("last scheduled run: never"),
            "{}",
            stdout(&o)
        );
    }
}

#[cfg(windows)]
#[test]
fn invalid_utf16_names_break_json_but_never_the_sweep() {
    use std::os::windows::ffi::OsStringExt;
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    // A project dir whose name contains an unpaired surrogate: its paths are
    // not representable as UTF-8, so JSON serialization must fail loudly —
    // while the sweep itself keeps working on raw OS paths.
    let weird: std::ffi::OsString =
        std::ffi::OsString::from_wide(&[0x77, 0x65, 0x69, 0x72, 0x64, 0xD800]);
    let proj = t.path().join(weird);
    let profile = proj.join("target/debug");
    for (hash, size) in [("aaaaaaaaaaaaaaaa", 500u32), ("bbbbbbbbbbbbbbbb", 900)] {
        let fp = profile.join(".fingerprint").join(format!("serde-{hash}"));
        write(&fp.join("lib-serde"), 16);
        write(&fp.join("lib-serde.json"), 32);
        write(
            &profile.join(format!("deps/libserde-{hash}.rlib")),
            size as usize,
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let root = t.path().to_str().unwrap().to_string();

    let o = run_in(&home, &["report", "--json", &root]);
    assert!(!o.status.success());

    let o = run_in(&home, &["gc", "--json", &root]);
    assert!(!o.status.success());

    // Apply: deletion succeeds; only the summary serialization complains.
    let o = run_in(&home, &["gc", "--apply", "--json", &root]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("failed to serialize"));
    assert!(!profile.join("deps/libserde-aaaaaaaaaaaaaaaa.rlib").exists());
}

#[cfg(windows)]
#[test]
fn schedule_powershell_failure_paths() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let run = |args: &[&str]| {
        Command::new(bin())
            .args(args)
            .env("CARGO_HOME", &home)
            .env("TARGONE_TASK_NAME", "TargoneCovNever")
            .env("TARGONE_PS_BIN", "definitely-not-a-real-shell")
            .output()
            .unwrap()
    };
    assert!(!run(&["schedule", "install"]).status.success());
    let o = run(&["schedule", "status"]);
    assert!(stdout(&o).contains("error"), "{}", stdout(&o));

    // An interpreter that RUNS but rejects the arguments (nonzero + stderr):
    // the script-failure Err path.
    let o = Command::new(bin())
        .args(["schedule", "status"])
        .env("CARGO_HOME", &home)
        .env("TARGONE_TASK_NAME", "TargoneCovNever")
        .env("TARGONE_PS_BIN", "cargo")
        .output()
        .unwrap();
    assert!(stdout(&o).contains("error"), "{}", stdout(&o));
}
