//! Profile-directory layout probing.
//!
//! Grammar established empirically in spikes/05-layout-detection.md: three
//! layouts exist in the wild (legacy unified, `build.build-dir` split, and
//! nightly layout v2). Classification requires a positive match; anything else
//! is `Unknown` and must never be swept (fail-closed).

use std::fs;
use std::path::Path;

use serde::Serialize;

/// The Cargo `CACHEDIR.TAG` signature line (also what `cargo clean` validates
/// since 1.96 before agreeing to delete a directory).
pub const CACHEDIR_TAG_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileLayout {
    /// `.fingerprint/` + `deps/` present: unified layout, or the build-dir
    /// half of a `build.build-dir` split.
    LegacyBuild,
    /// `build/<pkg>/<16hex>/fingerprint/` present, no `.fingerprint/`/`deps/`:
    /// layout v2 (`-Zbuild-dir-new-layout`, default from ~Cargo 1.99).
    V2,
    /// Artifact-dir half of a split: uplifted artifacts + `.cargo-artifact-lock`,
    /// none of the build pools. Nothing intra-profile to sweep here.
    ArtifactOnly,
    /// No grammar matched. Sweep nothing, report.
    Unknown,
}

/// Probe one profile directory (e.g. `target/debug`).
pub fn probe_profile(dir: &Path) -> ProfileLayout {
    let has = |name: &str| dir.join(name).is_dir();
    if has(".fingerprint") && has("deps") {
        return ProfileLayout::LegacyBuild;
    }
    if !has(".fingerprint") && !has("deps") && has_v2_unit(&dir.join("build")) {
        return ProfileLayout::V2;
    }
    if dir.join(".cargo-artifact-lock").is_file() || dir.join(".cargo-lock").is_file() {
        return ProfileLayout::ArtifactOnly;
    }
    ProfileLayout::Unknown
}

/// True if `build_dir` contains at least one `<pkg>/<16hex>/fingerprint` chain.
fn has_v2_unit(build_dir: &Path) -> bool {
    let Ok(pkgs) = fs::read_dir(build_dir) else {
        return false;
    };
    for pkg in pkgs.flatten() {
        let Ok(metas) = fs::read_dir(pkg.path()) else {
            continue;
        };
        for meta in metas.flatten() {
            let name = meta.file_name();
            let is_hash = name.to_str().is_some_and(crate::unit::is_unit_hash);
            if is_hash && meta.path().join("fingerprint").is_dir() {
                return true;
            }
        }
    }
    false
}

/// True if `dir/CACHEDIR.TAG` exists as a regular file with the Cargo
/// signature. A positive discovery signal only — its absence proves nothing
/// (analysis F-055: dirs created by rust-analyzer first may lack it).
pub fn has_cachedir_tag(dir: &Path) -> bool {
    let path = dir.join("CACHEDIR.TAG");
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    match fs::read(&path) {
        Ok(bytes) => bytes.starts_with(CACHEDIR_TAG_SIGNATURE),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn mk(root: &Path, entries: &[&str]) {
        for e in entries {
            if let Some(stripped) = e.strip_suffix('/') {
                fs::create_dir_all(root.join(stripped)).unwrap();
            } else {
                if let Some(parent) = root.join(e).parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(root.join(e), b"").unwrap();
            }
        }
    }

    #[test]
    fn legacy_layout_is_detected() {
        let t = tempfile::tempdir().unwrap();
        mk(
            t.path(),
            &[".fingerprint/", "deps/", "build/", "incremental/"],
        );
        assert_eq!(probe_profile(t.path()), ProfileLayout::LegacyBuild);
    }

    #[test]
    fn v2_layout_is_detected() {
        let t = tempfile::tempdir().unwrap();
        mk(
            t.path(),
            &["build/serde/8cf73098e091883d/fingerprint/", "incremental/"],
        );
        assert_eq!(probe_profile(t.path()), ProfileLayout::V2);
    }

    #[test]
    fn artifact_only_side_is_detected() {
        let t = tempfile::tempdir().unwrap();
        mk(
            t.path(),
            &["examples/", ".cargo-artifact-lock", ".cargo-lock"],
        );
        assert_eq!(probe_profile(t.path()), ProfileLayout::ArtifactOnly);
    }

    #[test]
    fn unmatched_dir_is_unknown() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), &["src/", "README.md"]);
        assert_eq!(probe_profile(t.path()), ProfileLayout::Unknown);
    }

    #[test]
    fn v2_requires_hash_and_fingerprint() {
        let t = tempfile::tempdir().unwrap();
        // build/ exists but children don't match <pkg>/<16hex>/fingerprint
        mk(t.path(), &["build/serde/not-a-hash/fingerprint/"]);
        assert_eq!(probe_profile(t.path()), ProfileLayout::Unknown);
    }

    #[test]
    fn v2_probe_skips_files_and_junk_at_pkg_level() {
        let t = tempfile::tempdir().unwrap();
        mk(t.path(), &["build/stray-file", "build/serde/junk-name/"]);
        assert_eq!(probe_profile(t.path()), ProfileLayout::Unknown);
    }

    #[test]
    fn cachedir_tag_must_be_a_regular_file() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("CACHEDIR.TAG")).unwrap();
        assert!(!has_cachedir_tag(t.path()));
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_cachedir_tag_is_not_a_marker() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        let tag = t.path().join("CACHEDIR.TAG");
        fs::write(&tag, b"Signature: 8a477f597d28d172789f06886806bc55").unwrap();
        let _holder = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&tag)
            .unwrap();
        assert!(!has_cachedir_tag(t.path()));
    }

    #[test]
    fn cachedir_tag_signature_is_verified() {
        let t = tempfile::tempdir().unwrap();
        assert!(!has_cachedir_tag(t.path()));
        fs::write(t.path().join("CACHEDIR.TAG"), b"Signature: wrong").unwrap();
        assert!(!has_cachedir_tag(t.path()));
        fs::write(
            t.path().join("CACHEDIR.TAG"),
            b"Signature: 8a477f597d28d172789f06886806bc55\n# comment",
        )
        .unwrap();
        assert!(has_cachedir_tag(t.path()));
    }
}
