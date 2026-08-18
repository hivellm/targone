//! # targone — the beacon crate
//!
//! Add this as a build-dependency (or dependency) and your project registers
//! itself with the [Targone engine](https://crates.io/crates/cargo-targone),
//! which garbage-collects superseded build artifacts across your machine on
//! a schedule — under Cargo's own file locks, keeping warm builds warm.
//!
//! ```toml
//! [build-dependencies]
//! targone = "0.1"
//! ```
//!
//! ## Trust contract
//!
//! The build script's single side effect is appending one JSON line
//! (`{"v":1,"root":"<your project>","ts":<unix>}`) to the local file
//! `$CARGO_HOME/targone/registry.jsonl`, plus at most one `cargo:warning`
//! per day when the engine is not installed. It never deletes anything,
//! never spawns a process, never touches the network, and is a hard no-op
//! under `DOCS_RS`, `TARGONE_DISABLE=1`, or `CI`. The whole implementation
//! is [`build.rs`] + one included module — auditable in five minutes.
//!
//! This crate exposes no runtime API.

#[cfg(test)]
#[path = "beacon_impl.rs"]
mod beacon_impl;

#[cfg(test)]
mod tests {
    use super::beacon_impl::*;
    use std::ffi::OsString;
    use std::fs;

    #[test]
    fn resolves_target_root_and_project_from_out_dir() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("proj/target");
        let out = target.join("debug/build/targone-0123456789abcdef/out");
        fs::create_dir_all(&out).unwrap();
        fs::write(target.join(".rustc_info.json"), b"{}").unwrap();
        let root = target_root_from_out_dir(&out).unwrap();
        assert_eq!(root, target);
        assert_eq!(project_root(&root), t.path().join("proj"));
    }

    #[test]
    fn renamed_build_dir_registers_as_itself() {
        let t = tempfile::tempdir().unwrap();
        let bdir = t.path().join("central-build");
        let out = bdir.join("debug/build/x-0123456789abcdef/out");
        fs::create_dir_all(&out).unwrap();
        fs::write(
            bdir.join("CACHEDIR.TAG"),
            b"Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        let root = target_root_from_out_dir(&out).unwrap();
        assert_eq!(project_root(&root), bdir);
    }

    #[test]
    fn unmarked_world_is_a_no_op() {
        let t = tempfile::tempdir().unwrap();
        let out = t.path().join("a/b/c/out");
        fs::create_dir_all(&out).unwrap();
        assert!(target_root_from_out_dir(&out).is_none());
    }

    #[test]
    fn registry_line_matches_engine_schema() {
        let line = registry_line(std::path::Path::new(r"E:\code\my proj"), 42);
        // The engine's serde-based reader must parse it.
        assert_eq!(line, r#"{"v":1,"root":"E:\\code\\my proj","ts":42}"#);
    }

    #[test]
    fn disabled_under_docs_rs_ci_and_optout() {
        let on =
            |k: &'static str, v: &'static str| move |q: &str| (q == k).then(|| OsString::from(v));
        assert!(beacon_disabled(on("DOCS_RS", "1")));
        assert!(beacon_disabled(on("CI", "true")));
        assert!(beacon_disabled(on("TARGONE_DISABLE", "1")));
        assert!(!beacon_disabled(|_| None));
        // TARGONE_DISABLE with another value does not disable.
        assert!(!beacon_disabled(on("TARGONE_DISABLE", "0")));
    }

    #[test]
    fn hint_fires_at_most_daily() {
        let t = tempfile::tempdir().unwrap();
        let stamp = t.path().join("hint.stamp");
        assert!(hint_due(&stamp, 1_000_000)); // no stamp yet
        fs::write(&stamp, b"").unwrap();
        let now = fs::metadata(&stamp)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(!hint_due(&stamp, now + 3600)); // an hour later: quiet
        assert!(hint_due(&stamp, now + 25 * 3600)); // a day later: due
    }

    #[test]
    fn cargo_home_resolution_order() {
        let on =
            |k: &'static str, v: &'static str| move |q: &str| (q == k).then(|| OsString::from(v));
        assert_eq!(
            cargo_home(on("CARGO_HOME", "/custom")),
            std::path::PathBuf::from("/custom")
        );
        assert_eq!(
            cargo_home(on("HOME", "/home/u")),
            std::path::PathBuf::from("/home/u").join(".cargo")
        );
        assert_eq!(
            cargo_home(|_| None),
            std::path::PathBuf::from(".").join(".cargo")
        );
    }

    #[test]
    fn engine_path_probe_never_spawns() {
        let t = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) {
            "cargo-targone.exe"
        } else {
            "cargo-targone"
        };
        assert!(!engine_on_path(Some(std::ffi::OsStr::new(
            t.path().to_str().unwrap()
        ))));
        fs::write(t.path().join(exe), b"").unwrap();
        assert!(engine_on_path(Some(t.path().as_os_str())));
        assert!(!engine_on_path(None));
    }
}
