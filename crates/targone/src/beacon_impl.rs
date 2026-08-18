//! Pure logic of the beacon build script, factored out so it can be unit
//! tested from `lib.rs` (the build script includes this file via `#[path]`).
//!
//! Trust contract (analysis F-062): everything here is fail-silent,
//! metadata-only, local-only. No process is ever spawned, nothing is ever
//! deleted, no network exists. The single side effect is appending one JSON
//! line to `$CARGO_HOME/targone/registry.jsonl` (plus, at most once per day,
//! a `cargo:warning` when the engine binary is missing).

use std::path::{Path, PathBuf};

/// The Cargo CACHEDIR.TAG signature (same constant the engine validates).
const CACHEDIR_SIG: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Walk up from `OUT_DIR` (…/<profile>/build/<pkg>-<hash>/out) to the
/// target/build-dir root, identified by a positive marker. Bounded ascent;
/// `None` means "unrecognized world — do nothing" (fail-silent).
pub fn target_root_from_out_dir(out_dir: &Path) -> Option<PathBuf> {
    let mut dir = out_dir.to_path_buf();
    for _ in 0..8 {
        if is_target_root(&dir) {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

fn is_target_root(dir: &Path) -> bool {
    if dir.join(".rustc_info.json").is_file() {
        return true;
    }
    let tag = dir.join("CACHEDIR.TAG");
    match std::fs::read(&tag) {
        Ok(bytes) => bytes.starts_with(CACHEDIR_SIG),
        Err(_) => false,
    }
}

/// The project root recorded in the registry: conventional `X/target`
/// belongs to `X`; a renamed/central build dir registers as itself
/// (identical rule to the engine's `scan` command).
pub fn project_root(target_root: &Path) -> PathBuf {
    if target_root
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("target"))
    {
        target_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| target_root.to_path_buf())
    } else {
        target_root.to_path_buf()
    }
}

/// One registry line, schema-compatible with the engine
/// (`{"v":1,"root":…,"ts":…}`), hand-rolled because this crate carries zero
/// dependencies.
pub fn registry_line(root: &Path, ts: u64) -> String {
    format!(
        "{{\"v\":1,\"root\":\"{}\",\"ts\":{ts}}}",
        json_escape(&root.to_string_lossy())
    )
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `$CARGO_HOME` resolution without any dependency (same rule as the engine).
pub fn cargo_home(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
    if let Some(h) = env("CARGO_HOME") {
        return PathBuf::from(h);
    }
    let home = env("USERPROFILE")
        .or_else(|| env("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cargo")
}

/// True when the beacon must be a hard no-op: docs.rs (read-only sandbox),
/// explicit opt-out, or CI (registering ephemeral runner paths would poison
/// the registry).
pub fn beacon_disabled(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    env("DOCS_RS").is_some()
        || env("TARGONE_DISABLE").is_some_and(|v| v == "1")
        || env("CI").is_some()
}

/// Is `cargo-targone` reachable on PATH? File-existence probe only — the
/// beacon never spawns a process.
pub fn engine_on_path(path_var: Option<&std::ffi::OsStr>) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    #[cfg(windows)]
    const EXE: &str = "cargo-targone.exe";
    #[cfg(not(windows))]
    const EXE: &str = "cargo-targone";
    std::env::split_paths(path_var).any(|dir| dir.join(EXE).is_file())
}

/// Should the engine-missing hint fire? At most once per day, stamped by the
/// mtime of `hint.stamp` (F-062: the beacon must never nag).
pub fn hint_due(stamp: &Path, now_secs: u64) -> bool {
    match std::fs::metadata(stamp).and_then(|m| m.modified()) {
        Ok(mtime) => {
            let stamp_secs = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            now_secs.saturating_sub(stamp_secs) > 24 * 3600
        }
        Err(_) => true,
    }
}
