//! Metadata-only enumeration and identity-recency classification.
//!
//! The primary rule is F-044: *keep the newest N per identity key* — no
//! wall-clock cliffs. The identity key, refined by spikes 0.3/0.4, is
//! `(package, unit-state-file set, artifact class)`:
//!
//! - the state-file set separates unit kinds and multi-bin groupings;
//! - the artifact class (which build/deps/none artifacts the hash owns, and
//!   with which extensions) separates check-mode from build-mode fingerprints
//!   of the same unit — both live, never generations of each other;
//! - hash-absent ("orphan") fingerprints group among themselves only, keeping
//!   the newest: on MSVC these are the LIVE fingerprints of plain binaries,
//!   whose artifacts are hash-less (spike 0.3).
//!
//! Scanning never opens artifact file contents (F-056) — sizes, names and
//! mtimes only.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use walkdir::WalkDir;

use crate::discover::TargetDir;
use crate::layout::ProfileLayout;
use crate::unit::{
    artifact_hash, extension_chain, incremental_group, split_hash_suffix, unit_kind_from_files,
    unit_state_files, UnitKind,
};

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct PoolStats {
    pub bytes: u64,
    pub files: u64,
}

/// Reclaim tiers, cheapest/safest first (F-049).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// Superseded `incremental/<crate>-<disambig>` directories (keep newest 1).
    Incremental,
    /// Superseded compiled-unit artifacts: legacy `deps/`+`examples/` files +
    /// their fingerprint dirs, or whole v2 unit dirs.
    Units,
    /// Superseded build-script directories under `build/` + their fingerprints.
    BuildScripts,
    /// Superseded hash-absent fingerprint dirs (no artifacts to pair with),
    /// keep-newest within the orphan class only (spike 0.3 rule c.2).
    OrphanFingerprints,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct TierEstimate {
    pub tier: Tier,
    pub reclaimable_bytes: u64,
    pub reclaimable_entries: u64,
}

#[derive(Debug, Serialize)]
pub struct ProfileReport {
    pub path: PathBuf,
    pub layout: ProfileLayout,
    pub pools: BTreeMap<String, PoolStats>,
    pub tiers: Vec<TierEstimate>,
    /// Entries that matched no grammar and were therefore kept (fail-open).
    pub unparsed_kept: u64,
}

impl ProfileReport {
    pub fn total_bytes(&self) -> u64 {
        self.pools.values().map(|p| p.bytes).sum()
    }
    pub fn reclaimable_bytes(&self) -> u64 {
        self.tiers.iter().map(|t| t.reclaimable_bytes).sum()
    }
}

#[derive(Debug, Serialize)]
pub struct TargetReport {
    pub root: PathBuf,
    pub profiles: Vec<ProfileReport>,
    /// Root-level pools outside any profile (`doc/`, `package/`, stray files).
    pub root_pools: BTreeMap<String, PoolStats>,
}

impl TargetReport {
    pub fn total_bytes(&self) -> u64 {
        self.profiles.iter().map(|p| p.total_bytes()).sum::<u64>()
            + self.root_pools.values().map(|p| p.bytes).sum::<u64>()
    }
    pub fn reclaimable_bytes(&self) -> u64 {
        self.profiles.iter().map(|p| p.reclaimable_bytes()).sum()
    }
}

/// Scan one discovered target dir into a report. Read-only.
pub fn scan_target_dir(td: &TargetDir) -> TargetReport {
    let profiles = td
        .profiles
        .iter()
        .map(|(path, layout)| scan_profile(path, *layout))
        .collect();
    let mut root_pools = BTreeMap::new();
    for extra in ["doc", "package"] {
        let p = td.root.join(extra);
        if p.is_dir() {
            root_pools.insert(extra.to_string(), dir_stats(&p));
        }
    }
    TargetReport {
        root: td.root.clone(),
        profiles,
        root_pools,
    }
}

/// Scan one profile directory. Read-only.
pub fn scan_profile(path: &Path, layout: ProfileLayout) -> ProfileReport {
    let mut report = ProfileReport {
        path: path.to_path_buf(),
        layout,
        pools: BTreeMap::new(),
        tiers: Vec::new(),
        unparsed_kept: 0,
    };
    collect_pools(path, &mut report);
    match layout {
        ProfileLayout::LegacyBuild => {
            classify_incremental(path, &mut report);
            classify_legacy_units(path, &mut report);
        }
        ProfileLayout::V2 => {
            classify_incremental(path, &mut report);
            classify_v2_units(path, &mut report);
        }
        ProfileLayout::ArtifactOnly | ProfileLayout::Unknown => {}
    }
    report
}

const PROFILE_POOLS: &[&str] = &[".fingerprint", "deps", "build", "incremental", "examples"];

fn collect_pools(path: &Path, report: &mut ProfileReport) {
    for pool in PROFILE_POOLS {
        let p = path.join(pool);
        if p.is_dir() {
            report.pools.insert((*pool).to_string(), dir_stats(&p));
        }
    }
    // Uplifted artifacts and lock files live directly in the profile dir.
    let mut uplifted = PoolStats::default();
    if let Ok(entries) = fs::read_dir(path) {
        for e in entries.flatten() {
            if let Ok(meta) = e.metadata() {
                if meta.is_file() {
                    uplifted.bytes += meta.len();
                    uplifted.files += 1;
                }
            }
        }
    }
    report.pools.insert("uplifted".to_string(), uplifted);
}

/// Tier 1: keep the newest incremental dir per crate name (F-003: measured
/// 96.9% of the pool on the reference machine).
fn classify_incremental(path: &Path, report: &mut ProfileReport) {
    let inc = path.join("incremental");
    if !inc.is_dir() {
        return;
    }
    let mut groups: BTreeMap<String, Vec<(SystemTime, u64)>> = BTreeMap::new();
    if let Ok(entries) = fs::read_dir(&inc) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_dir() {
                report.unparsed_kept += 1;
                continue;
            }
            match incremental_group(&name) {
                // Spike 0.4: every package's build script compiles to the
                // crate name `build_script_build` — same-name dirs here are
                // DIFFERENT packages, not generations. Keep them all.
                Some(("build_script_build", _)) => {}
                Some((krate, _)) => {
                    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    let size = dir_stats(&e.path()).bytes;
                    groups
                        .entry(krate.to_string())
                        .or_default()
                        .push((mtime, size));
                }
                None => report.unparsed_kept += 1,
            }
        }
    }
    let mut estimate = TierEstimate {
        tier: Tier::Incremental,
        reclaimable_bytes: 0,
        reclaimable_entries: 0,
    };
    for entries in groups.values_mut() {
        entries.sort_by_key(|(mtime, _)| *mtime);
        // Everything except the newest is dead the moment the new
        // disambiguator appeared (F-003).
        for (_, size) in entries.iter().take(entries.len().saturating_sub(1)) {
            estimate.reclaimable_bytes += size;
            estimate.reclaimable_entries += 1;
        }
    }
    report.tiers.push(estimate);
}

/// Which artifacts a fingerprint hash owns — the mode discriminator.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ArtifactClass {
    /// Hash matches a `build/<pkg>-<hash>` directory (build-script units).
    BuildDir,
    /// Hash matches files in `deps/`/`examples/`; the extension-chain set
    /// separates check-mode (`{d, rmeta}`) from build-mode (`{d, rlib, rmeta}`)
    /// generations — distinct live identities, never superseded pairs.
    Deps(Vec<String>),
    /// Hash matches nothing: on MSVC, the live fingerprints of plain binaries
    /// (hash-less artifacts). Group among themselves only.
    Orphan,
}

struct FingerprintUnit {
    kind: UnitKind,
    recency: SystemTime,
    dir_bytes: u64,
    artifact_bytes: u64,
    artifact_files: u64,
    class_is_orphan: bool,
}

fn tally(tiers: &mut [TierEstimate], tier: Tier, bytes: u64, entries: u64) {
    if let Some(t) = tiers.iter_mut().find(|t| t.tier == tier) {
        t.reclaimable_bytes += bytes;
        t.reclaimable_entries += entries;
    }
}

/// Tiers 2–4 on the legacy layout: identity-recency over
/// `(package, state-file set, artifact class)` groups from `.fingerprint/`,
/// paired with `deps/`/`examples/` files and `build/` dirs by the shared hash
/// namespace (F-005). Fingerprints are only ever reclaimed together with the
/// artifacts of the same hash (spike 0.3 rule c.1).
fn classify_legacy_units(path: &Path, report: &mut ProfileReport) {
    let fingerprint_root = path.join(".fingerprint");
    let Ok(entries) = fs::read_dir(&fingerprint_root) else {
        return;
    };

    // Artifact index: hash → (bytes, files, extension-chain set).
    let mut deps_index: BTreeMap<String, (u64, u64, BTreeSet<String>)> = BTreeMap::new();
    for pool in ["deps", "examples"] {
        let Ok(files) = fs::read_dir(path.join(pool)) else {
            continue;
        };
        for e in files.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            // Hash-less files are the known plain-bin class: always kept.
            if let Some(h) = artifact_hash(&name) {
                let entry = deps_index.entry(h.to_string()).or_default();
                entry.0 += meta.len();
                entry.1 += 1;
                entry.2.insert(extension_chain(&name).to_string());
            }
        }
    }

    // build/ index: hash → recursive dir bytes.
    let mut build_index: BTreeMap<String, u64> = BTreeMap::new();
    if let Ok(dirs) = fs::read_dir(path.join("build")) {
        for e in dirs.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !e.path().is_dir() {
                continue;
            }
            match split_hash_suffix(&name) {
                Some((_, h)) => {
                    build_index.insert(h.to_string(), dir_stats(&e.path()).bytes);
                }
                None => report.unparsed_kept += 1,
            }
        }
    }

    // Group fingerprints by (package, state-file set, artifact class).
    type Key = (String, Vec<String>, ArtifactClass);
    let mut groups: BTreeMap<Key, Vec<FingerprintUnit>> = BTreeMap::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.path().is_dir() {
            report.unparsed_kept += 1;
            continue;
        }
        let Some((pkg, hash)) = split_hash_suffix(&name) else {
            report.unparsed_kept += 1;
            continue;
        };
        let mut files = Vec::new();
        let mut recency = SystemTime::UNIX_EPOCH;
        let mut dir_bytes = 0u64;
        if let Ok(inner) = fs::read_dir(e.path()) {
            for f in inner.flatten() {
                files.push(f.file_name().to_string_lossy().into_owned());
                if let Ok(meta) = f.metadata() {
                    dir_bytes += meta.len();
                    if let Ok(m) = meta.modified() {
                        recency = recency.max(m);
                    }
                }
            }
        }
        let Some(kind) = unit_kind_from_files(files.iter().map(String::as_str)) else {
            report.unparsed_kept += 1;
            continue;
        };
        let state = unit_state_files(files.iter().map(String::as_str));
        let (class, artifact_bytes, artifact_files) = if let Some(b) = build_index.get(hash) {
            (ArtifactClass::BuildDir, *b, 1)
        } else if let Some((bytes, count, exts)) = deps_index.get(hash) {
            (
                ArtifactClass::Deps(exts.iter().cloned().collect()),
                *bytes,
                *count,
            )
        } else {
            (ArtifactClass::Orphan, 0, 0)
        };
        let class_is_orphan = class == ArtifactClass::Orphan;
        groups
            .entry((pkg.to_string(), state, class))
            .or_default()
            .push(FingerprintUnit {
                kind,
                recency,
                dir_bytes,
                artifact_bytes,
                artifact_files,
                class_is_orphan,
            });
    }

    let mut tiers = [
        TierEstimate {
            tier: Tier::Units,
            reclaimable_bytes: 0,
            reclaimable_entries: 0,
        },
        TierEstimate {
            tier: Tier::BuildScripts,
            reclaimable_bytes: 0,
            reclaimable_entries: 0,
        },
        TierEstimate {
            tier: Tier::OrphanFingerprints,
            reclaimable_bytes: 0,
            reclaimable_entries: 0,
        },
    ];
    for units in groups.values_mut() {
        units.sort_by_key(|u| u.recency);
        for u in units.iter().take(units.len().saturating_sub(1)) {
            let tier = if u.class_is_orphan {
                Tier::OrphanFingerprints
            } else if matches!(
                u.kind,
                UnitKind::BuildScriptCompile | UnitKind::BuildScriptRun
            ) {
                Tier::BuildScripts
            } else {
                Tier::Units
            };
            // Fingerprint dir + its paired artifacts, reclaimed together.
            tally(
                &mut tiers,
                tier,
                u.dir_bytes + u.artifact_bytes,
                1 + u.artifact_files,
            );
        }
    }
    report.tiers.extend(tiers);
}

/// One v2 unit observation.
struct V2Unit {
    recency: SystemTime,
    dir_bytes: u64,
    is_build_script: bool,
}

/// Tiers 2–3 on layout v2: superseded whole unit dirs `build/<pkg>/<META>/`,
/// grouped by `(package, state-file set, out-extension set)` — the same
/// mode-aware identity as legacy, with the unit dir as the atomic artifact.
fn classify_v2_units(path: &Path, report: &mut ProfileReport) {
    let build_root = path.join("build");
    type Key = (String, Vec<String>, Vec<String>);
    let mut groups: BTreeMap<Key, Vec<V2Unit>> = BTreeMap::new();
    let Ok(pkgs) = fs::read_dir(&build_root) else {
        return;
    };
    for pkg in pkgs.flatten() {
        if !pkg.path().is_dir() {
            report.unparsed_kept += 1;
            continue;
        }
        let pkg_name = pkg.file_name().to_string_lossy().into_owned();
        let Ok(metas) = fs::read_dir(pkg.path()) else {
            continue;
        };
        for meta_dir in metas.flatten() {
            let hash_name = meta_dir.file_name().to_string_lossy().into_owned();
            let fp = meta_dir.path().join("fingerprint");
            if !crate::unit::is_unit_hash(&hash_name) || !fp.is_dir() {
                report.unparsed_kept += 1;
                continue;
            }
            let mut files = Vec::new();
            let mut recency = SystemTime::UNIX_EPOCH;
            if let Ok(inner) = fs::read_dir(&fp) {
                for f in inner.flatten() {
                    files.push(f.file_name().to_string_lossy().into_owned());
                    if let Ok(m) = f.metadata().and_then(|m| m.modified()) {
                        recency = recency.max(m);
                    }
                }
            }
            let Some(kind) = unit_kind_from_files(files.iter().map(String::as_str)) else {
                report.unparsed_kept += 1;
                continue;
            };
            let state = unit_state_files(files.iter().map(String::as_str));
            // Mode discriminator: the extension set of the unit's outputs.
            let mut exts: BTreeSet<String> = BTreeSet::new();
            if let Ok(out) = fs::read_dir(meta_dir.path().join("out")) {
                for f in out.flatten() {
                    let name = f.file_name().to_string_lossy().into_owned();
                    exts.insert(extension_chain(&name).to_string());
                }
            }
            if meta_dir.path().join("run").is_dir() {
                exts.insert("run/".to_string());
            }
            let size = dir_stats(&meta_dir.path()).bytes;
            let is_build_script = matches!(
                kind,
                UnitKind::BuildScriptCompile | UnitKind::BuildScriptRun
            );
            groups
                .entry((pkg_name.clone(), state, exts.into_iter().collect()))
                .or_default()
                .push(V2Unit {
                    recency,
                    dir_bytes: size,
                    is_build_script,
                });
        }
    }
    let mut tiers = [
        TierEstimate {
            tier: Tier::Units,
            reclaimable_bytes: 0,
            reclaimable_entries: 0,
        },
        TierEstimate {
            tier: Tier::BuildScripts,
            reclaimable_bytes: 0,
            reclaimable_entries: 0,
        },
    ];
    for entries in groups.values_mut() {
        entries.sort_by_key(|u| u.recency);
        for u in entries.iter().take(entries.len().saturating_sub(1)) {
            let tier = if u.is_build_script {
                Tier::BuildScripts
            } else {
                Tier::Units
            };
            tally(&mut tiers, tier, u.dir_bytes, 1);
        }
    }
    report.tiers.extend(tiers);
}

/// Recursive size/count via metadata only. Symlinks are counted, not followed.
pub fn dir_stats(path: &Path) -> PoolStats {
    let mut stats = PoolStats::default();
    for entry in WalkDir::new(path).follow_links(false).into_iter().flatten() {
        if entry.file_type().is_file() {
            if let Ok(meta) = entry.metadata() {
                stats.bytes += meta.len();
                stats.files += 1;
            }
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread::sleep;
    use std::time::Duration;

    fn write(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0u8; bytes]).unwrap();
    }

    /// Create a legacy lib unit: fingerprint dir + rlib in deps/.
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
    fn legacy_superseded_lib_is_reclaimable_newest_kept() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        sleep(Duration::from_millis(60));
        lib_unit(&profile, "serde", "bbbbbbbbbbbbbbbb", 2000);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let units = report.tiers.iter().find(|t| t.tier == Tier::Units).unwrap();
        // Old rlib (1000) + old fingerprint dir (16+32+8) reclaimable.
        assert_eq!(units.reclaimable_bytes, 1000 + 56);
        assert_eq!(units.reclaimable_entries, 2);
    }

    #[test]
    fn single_hash_units_are_never_reclaimable() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        lib_unit(&profile, "itoa", "cccccccccccccccc", 500);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        assert_eq!(report.reclaimable_bytes(), 0);
    }

    #[test]
    fn different_kinds_of_same_package_are_distinct_identities() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        // A lib unit and a build-script-run unit of the same package must not
        // supersede each other even with different hashes.
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        sleep(Duration::from_millis(60));
        let fp = profile.join(".fingerprint").join("serde-dddddddddddddddd");
        write(&fp.join("run-build-script-build-script-build"), 16);
        write(&fp.join("run-build-script-build-script-build.json"), 32);
        fs::create_dir_all(profile.join("deps")).unwrap();
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        assert_eq!(report.reclaimable_bytes(), 0);
    }

    #[test]
    fn check_and_build_modes_are_distinct_identities() {
        // Spike 0.3 finding 4: a check-mode fingerprint (rmeta-only artifacts)
        // arriving AFTER a build-mode one must not supersede it — both live.
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000); // build mode: .rlib
        sleep(Duration::from_millis(60));
        // check mode: same state files, artifacts are .rmeta + .d only.
        let fp = profile.join(".fingerprint").join("serde-eeeeeeeeeeeeeeee");
        write(&fp.join("lib-serde"), 16);
        write(&fp.join("lib-serde.json"), 32);
        write(&fp.join("invoked.timestamp"), 8);
        write(&profile.join("deps/libserde-eeeeeeeeeeeeeeee.rmeta"), 500);
        write(&profile.join("deps/serde-eeeeeeeeeeeeeeee.d"), 50);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        assert_eq!(report.reclaimable_bytes(), 0);
    }

    #[test]
    fn orphan_fingerprints_keep_newest_within_orphan_class() {
        // Spike 0.3 rule c.2: hash-absent fingerprints (MSVC plain bins)
        // group among themselves; the newest is the LIVE one.
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        fs::create_dir_all(profile.join("deps")).unwrap();
        let old = profile.join(".fingerprint").join("app-aaaaaaaaaaaaaaaa");
        write(&old.join("bin-app"), 100);
        write(&old.join("bin-app.json"), 32);
        sleep(Duration::from_millis(60));
        let new = profile.join(".fingerprint").join("app-bbbbbbbbbbbbbbbb");
        write(&new.join("bin-app"), 100);
        write(&new.join("bin-app.json"), 32);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let orphans = report
            .tiers
            .iter()
            .find(|t| t.tier == Tier::OrphanFingerprints)
            .unwrap();
        assert_eq!(orphans.reclaimable_bytes, 132);
        assert_eq!(orphans.reclaimable_entries, 1);
        let units = report.tiers.iter().find(|t| t.tier == Tier::Units).unwrap();
        assert_eq!(units.reclaimable_bytes, 0);
    }

    #[test]
    fn incremental_keeps_newest_per_crate() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::create_dir_all(profile.join("deps")).unwrap();
        let inc = profile.join("incremental");
        write(&inc.join("mycrate-aaaa/s-x-y/dep-graph.bin"), 300);
        sleep(Duration::from_millis(60));
        write(&inc.join("mycrate-bbbb/s-x-z/dep-graph.bin"), 400);
        write(&inc.join("other_crate-cccc/s-x-w/dep-graph.bin"), 100);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let inc_tier = report
            .tiers
            .iter()
            .find(|t| t.tier == Tier::Incremental)
            .unwrap();
        assert_eq!(inc_tier.reclaimable_bytes, 300);
        assert_eq!(inc_tier.reclaimable_entries, 1);
    }

    #[test]
    fn build_script_build_incremental_dirs_are_never_grouped() {
        // Spike 0.4: same-name build_script_build dirs belong to DIFFERENT
        // packages; keep-newest-1 across them would delete live caches.
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::create_dir_all(profile.join("deps")).unwrap();
        let inc = profile.join("incremental");
        write(
            &inc.join("build_script_build-aaaa/s-x-y/dep-graph.bin"),
            300,
        );
        sleep(Duration::from_millis(60));
        write(
            &inc.join("build_script_build-bbbb/s-x-z/dep-graph.bin"),
            400,
        );
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let inc_tier = report
            .tiers
            .iter()
            .find(|t| t.tier == Tier::Incremental)
            .unwrap();
        assert_eq!(inc_tier.reclaimable_bytes, 0);
    }

    #[test]
    fn unparsed_incremental_entries_are_kept() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::create_dir_all(profile.join("deps")).unwrap();
        write(&profile.join("incremental/not a cargo name!/x"), 100);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let inc_tier = report
            .tiers
            .iter()
            .find(|t| t.tier == Tier::Incremental)
            .unwrap();
        assert_eq!(inc_tier.reclaimable_bytes, 0);
        assert!(report.unparsed_kept >= 1);
    }

    #[test]
    fn scan_never_opens_artifact_contents() {
        // F-056: atime is a policy signal; the scanner must not destroy it.
        // Metadata-only enumeration leaves accessed() untouched.
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 4096);
        let rlib = profile.join("deps/libserde-aaaaaaaaaaaaaaaa.rlib");
        let before = fs::metadata(&rlib).and_then(|m| m.accessed());
        let _ = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let after = fs::metadata(&rlib).and_then(|m| m.accessed());
        if let (Ok(b), Ok(a)) = (before, after) {
            assert_eq!(b, a, "scan must not update artifact atime");
        }
    }

    #[test]
    fn v2_superseded_unit_dirs_are_reclaimable() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        let old = profile.join("build/serde/aaaaaaaaaaaaaaaa");
        write(&old.join("fingerprint/lib-serde"), 16);
        write(&old.join("out/libserde-aaaaaaaaaaaaaaaa.rlib"), 1000);
        sleep(Duration::from_millis(60));
        let new = profile.join("build/serde/bbbbbbbbbbbbbbbb");
        write(&new.join("fingerprint/lib-serde"), 16);
        write(&new.join("out/libserde-bbbbbbbbbbbbbbbb.rlib"), 2000);
        let report = scan_profile(&profile, ProfileLayout::V2);
        let units = report.tiers.iter().find(|t| t.tier == Tier::Units).unwrap();
        assert_eq!(units.reclaimable_bytes, 1016);
        assert_eq!(units.reclaimable_entries, 1);
    }

    #[test]
    fn v2_check_and_build_modes_are_distinct_identities() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        let build_mode = profile.join("build/serde/aaaaaaaaaaaaaaaa");
        write(&build_mode.join("fingerprint/lib-serde"), 16);
        write(&build_mode.join("out/libserde-aaaaaaaaaaaaaaaa.rlib"), 1000);
        sleep(Duration::from_millis(60));
        let check_mode = profile.join("build/serde/bbbbbbbbbbbbbbbb");
        write(&check_mode.join("fingerprint/lib-serde"), 16);
        write(&check_mode.join("out/libserde-bbbbbbbbbbbbbbbb.rmeta"), 200);
        let report = scan_profile(&profile, ProfileLayout::V2);
        assert_eq!(report.reclaimable_bytes(), 0);
    }
}
