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
//! Classification produces concrete [`ReclaimItem`]s: what to delete, in
//! which order (fingerprint dir before its artifacts — a missing output is a
//! safe rebuild, a stale fingerprint over missing outputs is the hazard), and
//! which rustc session locks must be held. The sweep layer executes them;
//! this module never deletes anything.
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
    /// OPT-IN (tier 5, F-042): every `.pdb` under `deps/`/`build/` — terminal
    /// debug-symbol outputs nothing reads back; worst case is re-linking to
    /// regenerate symbols for already-built binaries.
    Pdb,
    /// OPT-IN (tier 6, F-043's one legitimate use of age): full pool wipe of
    /// a profile whose own newest compile is older than the cutoff.
    Dormant,
}

/// One concrete deletion unit. `delete_first` paths (directories) go before
/// `delete_then` paths (files/dirs) — the fingerprint-before-artifacts order.
/// If any `session_locks` file cannot be exclusively locked, the whole item
/// is skipped (rustc may be using the incremental session).
#[derive(Debug, Clone, Serialize)]
pub struct ReclaimItem {
    pub tier: Tier,
    pub delete_first: Vec<PathBuf>,
    pub delete_then: Vec<PathBuf>,
    pub session_locks: Vec<PathBuf>,
    pub bytes: u64,
    pub entries: u64,
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
    /// The concrete deletion plan behind the tier estimates.
    pub reclaim: Vec<ReclaimItem>,
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
        reclaim: Vec::new(),
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
    // Derive tier estimates from the concrete plan.
    let tier_set: &[Tier] = match layout {
        ProfileLayout::LegacyBuild => &[
            Tier::Incremental,
            Tier::Units,
            Tier::BuildScripts,
            Tier::OrphanFingerprints,
        ],
        ProfileLayout::V2 => &[Tier::Incremental, Tier::Units, Tier::BuildScripts],
        _ => &[],
    };
    for &tier in tier_set {
        let (bytes, entries) = report
            .reclaim
            .iter()
            .filter(|i| i.tier == tier)
            .fold((0u64, 0u64), |acc, i| (acc.0 + i.bytes, acc.1 + i.entries));
        report.tiers.push(TierEstimate {
            tier,
            reclaimable_bytes: bytes,
            reclaimable_entries: entries,
        });
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
    let mut groups: BTreeMap<String, Vec<(SystemTime, u64, PathBuf)>> = BTreeMap::new();
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
                        .push((mtime, size, e.path()));
                }
                None => report.unparsed_kept += 1,
            }
        }
    }
    for entries in groups.values_mut() {
        entries.sort_by_key(|e| e.0);
        // Everything except the newest is dead the moment the new
        // disambiguator appeared (F-003).
        for (_, size, dir) in entries.iter().take(entries.len().saturating_sub(1)) {
            // rustc coordinates sessions via `s-*.lock` files inside the dir;
            // the sweep must hold them exclusively before deleting (spike 0.4).
            let mut session_locks = Vec::new();
            if let Ok(inner) = fs::read_dir(dir) {
                for f in inner.flatten() {
                    let n = f.file_name().to_string_lossy().into_owned();
                    if n.ends_with(".lock") {
                        session_locks.push(f.path());
                    }
                }
            }
            report.reclaim.push(ReclaimItem {
                tier: Tier::Incremental,
                delete_first: vec![dir.clone()],
                delete_then: Vec::new(),
                session_locks,
                bytes: *size,
                entries: 1,
            });
        }
    }
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
    fp_dir: PathBuf,
    fp_bytes: u64,
    artifact_paths: Vec<PathBuf>,
    artifact_bytes: u64,
    artifact_files: u64,
    class_is_orphan: bool,
}

#[derive(Default)]
struct DepsEntry {
    bytes: u64,
    files: u64,
    exts: BTreeSet<String>,
    paths: Vec<PathBuf>,
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

    // Artifact index: hash → sizes, extension chains, concrete paths.
    let mut deps_index: BTreeMap<String, DepsEntry> = BTreeMap::new();
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
                entry.bytes += meta.len();
                entry.files += 1;
                entry.exts.insert(extension_chain(&name).to_string());
                entry.paths.push(e.path());
            }
        }
    }

    // build/ index: hash → (recursive dir bytes, path).
    let mut build_index: BTreeMap<String, (u64, PathBuf)> = BTreeMap::new();
    if let Ok(dirs) = fs::read_dir(path.join("build")) {
        for e in dirs.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !e.path().is_dir() {
                continue;
            }
            match split_hash_suffix(&name) {
                Some((_, h)) => {
                    build_index.insert(h.to_string(), (dir_stats(&e.path()).bytes, e.path()));
                }
                None => report.unparsed_kept += 1,
            }
        }
    }

    // Group fingerprints by (package, state-file set, artifact class).
    type Key = (String, Vec<String>, ArtifactClass);
    let mut groups: BTreeMap<Key, Vec<FingerprintUnit>> = BTreeMap::new();
    // Every hash that has a fingerprint dir at all (even unclassifiable ones
    // — those keep their artifacts, fail-open).
    let mut fingerprint_hashes: BTreeSet<String> = BTreeSet::new();
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
        fingerprint_hashes.insert(hash.to_string());
        let mut files = Vec::new();
        let mut recency = SystemTime::UNIX_EPOCH;
        let mut fp_bytes = 0u64;
        if let Ok(inner) = fs::read_dir(e.path()) {
            for f in inner.flatten() {
                files.push(f.file_name().to_string_lossy().into_owned());
                if let Ok(meta) = f.metadata() {
                    fp_bytes += meta.len();
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
        let (class, artifact_paths, artifact_bytes, artifact_files) =
            if let Some((b, p)) = build_index.get(hash) {
                (ArtifactClass::BuildDir, vec![p.clone()], *b, 1)
            } else if let Some(d) = deps_index.get(hash) {
                (
                    ArtifactClass::Deps(d.exts.iter().cloned().collect()),
                    d.paths.clone(),
                    d.bytes,
                    d.files,
                )
            } else {
                (ArtifactClass::Orphan, Vec::new(), 0, 0)
            };
        let class_is_orphan = class == ArtifactClass::Orphan;
        groups
            .entry((pkg.to_string(), state, class))
            .or_default()
            .push(FingerprintUnit {
                kind,
                recency,
                fp_dir: e.path(),
                fp_bytes,
                artifact_paths,
                artifact_bytes,
                artifact_files,
                class_is_orphan,
            });
    }

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
            report.reclaim.push(ReclaimItem {
                tier,
                // Fingerprint dir FIRST: a missing output makes the unit
                // stale (safe rebuild); a live-looking fingerprint over
                // missing outputs is the corruption hazard.
                delete_first: vec![u.fp_dir.clone()],
                delete_then: u.artifact_paths.clone(),
                session_locks: Vec::new(),
                bytes: u.fp_bytes + u.artifact_bytes,
                entries: 1 + u.artifact_files,
            });
        }
    }

    // Fingerprint-less hashed artifacts: Cargo never leaves an artifact
    // without its fingerprint (spike 0.3, A\F = 0), and the sweep runs under
    // the build lock, so no build is mid-flight here. These are dead weight
    // — typically residue of an interrupted sweep (fingerprint deleted,
    // artifact deletion failed) — and re-collecting them is what makes the
    // sweep re-runnable.
    for (hash, d) in &deps_index {
        if !fingerprint_hashes.contains(hash) {
            report.reclaim.push(ReclaimItem {
                tier: Tier::Units,
                delete_first: Vec::new(),
                delete_then: d.paths.clone(),
                session_locks: Vec::new(),
                bytes: d.bytes,
                entries: d.files,
            });
        }
    }
    for (hash, (bytes, path)) in &build_index {
        if !fingerprint_hashes.contains(hash) {
            report.reclaim.push(ReclaimItem {
                tier: Tier::BuildScripts,
                delete_first: vec![path.clone()],
                delete_then: Vec::new(),
                session_locks: Vec::new(),
                bytes: *bytes,
                entries: 1,
            });
        }
    }
}

/// One v2 unit observation.
struct V2Unit {
    recency: SystemTime,
    dir: PathBuf,
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
            if !crate::unit::is_unit_hash(&hash_name) {
                report.unparsed_kept += 1;
                continue;
            }
            if !fp.is_dir() {
                // Valid unit hash but no fingerprint half: residue of an
                // interrupted sweep — dead weight, re-collect (same
                // rationale as the legacy A\F rule).
                report.reclaim.push(ReclaimItem {
                    tier: Tier::Units,
                    delete_first: vec![meta_dir.path()],
                    delete_then: Vec::new(),
                    session_locks: Vec::new(),
                    bytes: dir_stats(&meta_dir.path()).bytes,
                    entries: 1,
                });
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
                    dir: meta_dir.path(),
                    dir_bytes: size,
                    is_build_script,
                });
        }
    }
    for entries in groups.values_mut() {
        entries.sort_by_key(|u| u.recency);
        for u in entries.iter().take(entries.len().saturating_sub(1)) {
            let tier = if u.is_build_script {
                Tier::BuildScripts
            } else {
                Tier::Units
            };
            report.reclaim.push(ReclaimItem {
                tier,
                delete_first: vec![u.dir.clone()],
                delete_then: Vec::new(),
                session_locks: Vec::new(),
                bytes: u.dir_bytes,
                entries: 1,
            });
        }
    }
}

/// Newest compile activity of a profile: max mtime over FILES beneath
/// `.fingerprint/` and `build/`. File mtimes deliberately, not directory
/// mtimes — our own sweeps disturb directory mtimes, and F-006 established
/// that fingerprint file mtimes mean "last actually compiled", which is
/// exactly the dormancy signal F-043 calls for. `None` = no evidence ⇒ the
/// caller must NOT treat the profile as dormant (fail-open).
pub fn newest_compile(profile: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    for base in [profile.join(".fingerprint"), profile.join("build")] {
        for entry in WalkDir::new(base)
            .max_depth(4)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if entry.file_type().is_file() {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(m) = meta.modified() {
                        newest = Some(newest.map_or(m, |n| n.max(m)));
                    }
                }
            }
        }
    }
    newest
}

/// Tier 5 (opt-in, F-042): plan every `.pdb` under `deps/` and `build/` that
/// the base plan does not already cover. Debug symbols are terminal outputs;
/// regenerating them costs a re-link, never a re-compile.
pub fn append_pdb_items(report: &mut ProfileReport) {
    if !matches!(
        report.layout,
        ProfileLayout::LegacyBuild | ProfileLayout::V2
    ) {
        return;
    }
    let planned_files: std::collections::BTreeSet<&Path> = report
        .reclaim
        .iter()
        .flat_map(|i| i.delete_then.iter().map(PathBuf::as_path))
        .collect();
    let planned_dirs: Vec<&Path> = report
        .reclaim
        .iter()
        .flat_map(|i| i.delete_first.iter().map(PathBuf::as_path))
        .collect();
    let mut files = Vec::new();
    let mut bytes = 0u64;
    for pool in ["deps", "build"] {
        for entry in WalkDir::new(report.path.join(pool))
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let is_pdb = entry
                .path()
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdb"));
            if !is_pdb {
                continue;
            }
            let p = entry.path();
            if planned_files.contains(p) || planned_dirs.iter().any(|d| p.starts_with(d)) {
                continue; // already reclaimed by the base plan
            }
            if let Ok(meta) = entry.metadata() {
                bytes += meta.len();
                files.push(p.to_path_buf());
            }
        }
    }
    if files.is_empty() {
        return;
    }
    let entries = files.len() as u64;
    report.reclaim.push(ReclaimItem {
        tier: Tier::Pdb,
        delete_first: Vec::new(),
        delete_then: files,
        session_locks: Vec::new(),
        bytes,
        entries,
    });
    report.tiers.push(TierEstimate {
        tier: Tier::Pdb,
        reclaimable_bytes: bytes,
        reclaimable_entries: entries,
    });
}

/// Lock and marker files that must survive any wipe (deleting a lock file we
/// hold open would fail on Windows anyway; markers identify the dir).
const KEEP_IN_PROFILE: &[&str] = &[
    ".cargo-lock",
    ".cargo-build-lock",
    ".cargo-artifact-lock",
    "CACHEDIR.TAG",
];

/// Tier 6 (opt-in, F-043): when the profile's own newest compile is at or
/// before `cutoff`, REPLACE the plan with one full-pool wipe (locks and
/// markers kept). Returns whether the profile was classified dormant.
/// No compile evidence at all ⇒ not dormant (fail-open).
pub fn append_dormant_item(report: &mut ProfileReport, cutoff: SystemTime) -> bool {
    if !matches!(
        report.layout,
        ProfileLayout::LegacyBuild | ProfileLayout::V2
    ) {
        return false;
    }
    let Some(newest) = newest_compile(&report.path) else {
        return false;
    };
    if newest > cutoff {
        return false;
    }
    let mut delete_first = Vec::new();
    let mut bytes = 0u64;
    let mut entries = 0u64;
    for pool in PROFILE_POOLS {
        let p = report.path.join(pool);
        if p.is_dir() {
            let stats = dir_stats(&p);
            bytes += stats.bytes;
            entries += 1;
            delete_first.push(p);
        }
    }
    let mut delete_then = Vec::new();
    if let Ok(children) = fs::read_dir(&report.path) {
        for e in children.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if KEEP_IN_PROFILE.contains(&name.as_str()) {
                continue;
            }
            if let Ok(meta) = e.metadata() {
                if meta.is_file() {
                    bytes += meta.len();
                    entries += 1;
                    delete_then.push(e.path());
                }
            }
        }
    }
    // The wipe supersedes every finer-grained item (their paths live inside
    // the pools being deleted) — replace, don't stack, so estimates stay
    // honest.
    report.reclaim.clear();
    report.tiers.clear();
    report.reclaim.push(ReclaimItem {
        tier: Tier::Dormant,
        delete_first,
        delete_then,
        session_locks: Vec::new(),
        bytes,
        entries,
    });
    report.tiers.push(TierEstimate {
        tier: Tier::Dormant,
        reclaimable_bytes: bytes,
        reclaimable_entries: entries,
    });
    true
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
    fn plan_orders_fingerprint_before_artifacts() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        sleep(Duration::from_millis(60));
        lib_unit(&profile, "serde", "bbbbbbbbbbbbbbbb", 2000);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        assert_eq!(report.reclaim.len(), 1);
        let item = &report.reclaim[0];
        assert!(item.delete_first[0].ends_with("serde-aaaaaaaaaaaaaaaa"));
        assert!(item.delete_then[0].ends_with("libserde-aaaaaaaaaaaaaaaa.rlib"));
    }

    #[test]
    fn single_hash_units_are_never_reclaimable() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        lib_unit(&profile, "itoa", "cccccccccccccccc", 500);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        assert_eq!(report.reclaimable_bytes(), 0);
        assert!(report.reclaim.is_empty());
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
    fn incremental_keeps_newest_per_crate_and_collects_session_locks() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::create_dir_all(profile.join("deps")).unwrap();
        let inc = profile.join("incremental");
        write(&inc.join("mycrate-aaaa/s-x-y-z/dep-graph.bin"), 300);
        write(&inc.join("mycrate-aaaa/s-x-y.lock"), 0);
        sleep(Duration::from_millis(60));
        write(&inc.join("mycrate-bbbb/s-x-w-z/dep-graph.bin"), 400);
        write(&inc.join("other_crate-cccc/s-x-v-z/dep-graph.bin"), 100);
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let inc_tier = report
            .tiers
            .iter()
            .find(|t| t.tier == Tier::Incremental)
            .unwrap();
        assert_eq!(inc_tier.reclaimable_bytes, 300);
        assert_eq!(inc_tier.reclaimable_entries, 1);
        let item = report
            .reclaim
            .iter()
            .find(|i| i.tier == Tier::Incremental)
            .unwrap();
        assert_eq!(item.session_locks.len(), 1);
        assert!(item.session_locks[0].ends_with("s-x-y.lock"));
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
    fn fingerprintless_artifacts_are_recollected() {
        // Residue of an interrupted sweep: artifacts whose fingerprint is
        // already gone must be reclaimable on the NEXT run (A\F = 0 is
        // Cargo's own invariant; under the build lock nothing is mid-build).
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "bbbbbbbbbbbbbbbb", 2000); // live, with fingerprint
        write(&profile.join("deps/libold-aaaaaaaaaaaaaaaa.rlib"), 700);
        write(
            &profile.join("build/old-cccccccccccccccc/build-script-build.exe"),
            300,
        );
        let report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let units = report.tiers.iter().find(|t| t.tier == Tier::Units).unwrap();
        assert_eq!(units.reclaimable_bytes, 700);
        let bs = report
            .tiers
            .iter()
            .find(|t| t.tier == Tier::BuildScripts)
            .unwrap();
        assert_eq!(bs.reclaimable_bytes, 300);
        // The live unit stays untouched.
        assert_eq!(report.reclaimable_bytes(), 1000);
    }

    #[test]
    fn v2_fingerprintless_unit_dir_is_recollected() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        let live = profile.join("build/serde/bbbbbbbbbbbbbbbb");
        write(&live.join("fingerprint/lib-serde"), 16);
        write(&live.join("out/libserde-bbbbbbbbbbbbbbbb.rlib"), 2000);
        let residue = profile.join("build/serde/aaaaaaaaaaaaaaaa");
        write(&residue.join("out/libserde-aaaaaaaaaaaaaaaa.rlib"), 500);
        let report = scan_profile(&profile, ProfileLayout::V2);
        assert_eq!(report.reclaimable_bytes(), 500);
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
        assert!(report.reclaim[0].delete_first[0].ends_with("aaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn pdb_tier_collects_only_unplanned_pdbs() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        // A superseded gen whose artifacts include a .pdb (already planned)…
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        write(&profile.join("deps/serde-aaaaaaaaaaaaaaaa.pdb"), 400);
        sleep(Duration::from_millis(60));
        lib_unit(&profile, "serde", "bbbbbbbbbbbbbbbb", 2000);
        // …and a live gen's pdb (not otherwise reclaimable).
        write(&profile.join("deps/serde-bbbbbbbbbbbbbbbb.pdb"), 700);
        write(
            &profile.join("build/x-cccccccccccccccc/build_script_build.pdb"),
            300,
        );
        let mut report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let base = report.reclaimable_bytes();
        append_pdb_items(&mut report);
        let pdb = report.tiers.iter().find(|t| t.tier == Tier::Pdb).unwrap();
        // Only the live pdb (700) + orphanless build pdb… note x-cccc has no
        // fingerprint → its whole dir is already planned by re-collection,
        // so only 700 lands in the Pdb tier.
        assert_eq!(pdb.reclaimable_bytes, 700);
        assert_eq!(report.reclaimable_bytes(), base + 700);
    }

    #[test]
    fn dormant_profile_is_wiped_wholesale_keeping_locks() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        lib_unit(&profile, "serde", "aaaaaaaaaaaaaaaa", 1000);
        write(&profile.join("incremental/x-aa/s-a-b-c/o.bin"), 200);
        write(&profile.join("app.exe"), 500);
        write(&profile.join(".cargo-build-lock"), 0);
        let mut report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        // Cutoff in the future ⇒ everything is older ⇒ dormant.
        let dormant =
            append_dormant_item(&mut report, SystemTime::now() + Duration::from_secs(3600));
        assert!(dormant);
        assert_eq!(report.reclaim.len(), 1);
        let item = &report.reclaim[0];
        assert_eq!(item.tier, Tier::Dormant);
        assert!(item.delete_then.iter().any(|p| p.ends_with("app.exe")));
        assert!(!item
            .delete_then
            .iter()
            .any(|p| p.ends_with(".cargo-build-lock")));
        // Fresh profile with a recent compile is NOT dormant.
        let mut fresh = scan_profile(&profile, ProfileLayout::LegacyBuild);
        let past_cutoff = SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
        assert!(!append_dormant_item(&mut fresh, past_cutoff));
    }

    #[test]
    fn empty_profile_is_never_dormant() {
        // No compile evidence ⇒ fail-open, never wipe.
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("debug");
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::create_dir_all(profile.join("deps")).unwrap();
        let mut report = scan_profile(&profile, ProfileLayout::LegacyBuild);
        assert!(!append_dormant_item(
            &mut report,
            SystemTime::now() + Duration::from_secs(3600)
        ));
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
