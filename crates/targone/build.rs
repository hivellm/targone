//! The Targone beacon. Total budget: one appended line in a local file.
//!
//! What this build script does, exhaustively:
//! 1. resolves the consumer's target root by walking up from `OUT_DIR`;
//! 2. appends `{"v":1,"root":…,"ts":…}` to `$CARGO_HOME/targone/registry.jsonl`;
//! 3. if `cargo-targone` is not on PATH, prints one `cargo:warning` hint —
//!    at most once per day (mtime stamp).
//!
//! What it never does: delete anything, spawn anything, touch the network,
//! emit `rerun-if-*` directives (so it runs ~once per target dir, F-019).
//! Hard no-op under `DOCS_RS`, `TARGONE_DISABLE=1`, or `CI`. Every failure
//! path is silent — this script must never break or slow a build.

#[path = "src/beacon_impl.rs"]
mod beacon_impl;

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Never propagate anything: a beacon failure is not a build concern.
    let _ = run();
}

fn run() -> Option<()> {
    let env = |k: &str| std::env::var_os(k);
    if beacon_impl::beacon_disabled(env) {
        return None;
    }
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR")?);
    let target_root = beacon_impl::target_root_from_out_dir(&out_dir)?;
    let project = beacon_impl::project_root(&target_root);

    let dir = beacon_impl::cargo_home(env).join("targone");
    std::fs::create_dir_all(&dir).ok()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = beacon_impl::registry_line(&project, now);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("registry.jsonl"))
        .ok()?;
    writeln!(f, "{line}").ok()?;

    if !beacon_impl::engine_on_path(std::env::var_os("PATH").as_deref()) {
        let stamp = dir.join("hint.stamp");
        if beacon_impl::hint_due(&stamp, now) {
            let _ = std::fs::write(&stamp, b"");
            println!(
                "cargo:warning=targone: project registered, but the engine is not installed — run: cargo install cargo-targone && cargo targone schedule install"
            );
        }
    }
    Some(())
}
