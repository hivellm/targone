//! Target-directory discovery under configured scan roots.
//!
//! Composite discriminator (analysis F-055 + spike 0.5): a directory is a
//! target/build root when it carries a positive marker (`.rustc_info.json`,
//! signed `CACHEDIR.TAG`) or the conventional name `target`, AND at least one
//! child (directly, or nested one level under a target-triple dir) probes as a
//! known profile layout. Symlinks and reparse points are never followed.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use crate::layout::{has_cachedir_tag, probe_profile, ProfileLayout};

/// A discovered target (or split build) directory with its profile dirs.
#[derive(Debug, Clone, Serialize)]
pub struct TargetDir {
    pub root: PathBuf,
    /// Profile directories that probed to a known layout, including ones
    /// nested under a target-triple directory.
    pub profiles: Vec<(PathBuf, ProfileLayout)>,
}

/// Directory names that never contain a target dir — pruned for speed.
const PRUNE: &[&str] = &[".git", ".svn", ".hg", "node_modules", ".rustup"];

/// Walk `roots` and return every genuine target/build root found.
/// Discovered roots are not descended into (a target dir cannot contain
/// another project's target dir; registry sources under `.cargo` are pruned
/// by the marker requirement anyway).
pub fn discover(roots: &[PathBuf]) -> Vec<TargetDir> {
    let mut found = Vec::new();
    for root in roots {
        let mut walker = WalkDir::new(root).follow_links(false).into_iter();
        while let Some(entry) = walker.next() {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if PRUNE.contains(&name.as_ref()) {
                walker.skip_current_dir();
                continue;
            }
            if let Some(td) = try_target_dir(entry.path()) {
                found.push(td);
                walker.skip_current_dir();
            }
        }
    }
    found
}

/// Probe a single directory as a candidate target/build root.
pub fn try_target_dir(dir: &Path) -> Option<TargetDir> {
    let named_target = dir
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("target"));
    let marked = named_target || dir.join(".rustc_info.json").is_file() || has_cachedir_tag(dir);
    if !marked {
        return None;
    }
    let profiles = collect_profiles(dir);
    (!profiles.is_empty()).then(|| TargetDir {
        root: dir.to_path_buf(),
        profiles,
    })
}

fn collect_profiles(root: &Path) -> Vec<(PathBuf, ProfileLayout)> {
    let mut profiles = Vec::new();
    let Ok(children) = fs::read_dir(root) else {
        return profiles;
    };
    for child in children.flatten() {
        let path = child.path();
        // DirEntry::file_type never follows symlinks — same guarantee as a
        // symlink_metadata probe, without a second syscall.
        if !child.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        match probe_profile(&path) {
            ProfileLayout::Unknown => {
                // One nesting level: `target/<triple>/<profile>` (cross builds).
                let grandchildren = fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|gc| gc.path())
                    .filter(|gp| gp.is_dir());
                for gp in grandchildren {
                    let layout = probe_profile(&gp);
                    if layout != ProfileLayout::Unknown {
                        profiles.push((gp, layout));
                    }
                }
            }
            layout => profiles.push((path, layout)),
        }
    }
    profiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"").unwrap();
    }

    fn legacy_profile(profile: &Path) {
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::create_dir_all(profile.join("deps")).unwrap();
    }

    #[test]
    fn finds_conventional_target_dir() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("proj/target");
        legacy_profile(&target.join("debug"));
        touch(&t.path().join("proj/src/main.rs"));
        let found = discover(&[t.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].root, target);
        assert_eq!(found[0].profiles.len(), 1);
    }

    #[test]
    fn finds_renamed_build_dir_via_rustc_info() {
        let t = tempfile::tempdir().unwrap();
        let bdir = t.path().join("central-build");
        touch(&bdir.join(".rustc_info.json"));
        legacy_profile(&bdir.join("debug"));
        let found = discover(&[t.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].root, bdir);
    }

    #[test]
    fn rejects_source_dir_named_target() {
        // The LLVM/GCC false-positive shape from F-055: a `Target/` full of
        // sources, no profile grammar inside.
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("llvm/lib/Target/X86/X86ISelLowering.cpp"));
        let found = discover(&[t.path().to_path_buf()]);
        assert!(found.is_empty());
    }

    #[test]
    fn finds_cross_compile_profiles_one_level_down() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("proj/target");
        legacy_profile(&target.join("debug"));
        legacy_profile(&target.join("x86_64-unknown-linux-gnu/release"));
        // Non-profile junk inside the triple dir is simply not a profile.
        fs::create_dir_all(target.join("x86_64-unknown-linux-gnu/junk")).unwrap();
        let found = discover(&[t.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].profiles.len(), 2);
    }

    #[test]
    fn pruned_dirs_are_never_entered() {
        let t = tempfile::tempdir().unwrap();
        // A marker-bearing target dir hidden inside node_modules must stay
        // invisible: the prune fires before the probe.
        let hidden = t.path().join("node_modules/evil/target");
        legacy_profile(&hidden.join("debug"));
        let found = discover(&[t.path().to_path_buf()]);
        assert!(found.is_empty());
    }

    #[test]
    fn a_file_named_target_is_not_a_target_dir() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("target");
        fs::write(&f, b"just a file").unwrap();
        assert!(try_target_dir(&f).is_none());
    }

    #[test]
    fn does_not_descend_into_found_target_dirs() {
        let t = tempfile::tempdir().unwrap();
        let outer = t.path().join("proj/target");
        legacy_profile(&outer.join("debug"));
        // A nested marker inside the found dir must not produce a second hit.
        touch(&outer.join("debug/build/x-0000000000000000/out/target/.rustc_info.json"));
        let found = discover(&[t.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
    }
}
