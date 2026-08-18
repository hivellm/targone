//! Unit identity parsing: the 16-hex hash namespace shared between
//! `.fingerprint/<pkg>-<hash>/` and `-C extra-filename` suffixes (analysis
//! F-005), fingerprint unit kinds, and `incremental/` directory grammar
//! (spike 0.4).

use serde::Serialize;

/// True for Cargo's unit hash: exactly 16 lowercase hex digits.
pub fn is_unit_hash(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Split `<stem>-<16hex>` into `(stem, hash)`. Returns `None` (⇒ keep,
/// fail-open) when the name carries no hash suffix.
pub fn split_hash_suffix(name: &str) -> Option<(&str, &str)> {
    let (stem, hash) = name.rsplit_once('-')?;
    is_unit_hash(hash).then_some((stem, hash))
}

/// Extract the unit hash from an artifact file name: take the part before the
/// first `.` (drops `.rlib`, `.dll.lib`, …), then the trailing `-<16hex>`.
/// Hash-less names are a *known* class, not noise: MSVC plain-bin outputs
/// (`cortex_api.exe`) carry no hash (spike 0.3) and are always kept.
pub fn artifact_hash(file_name: &str) -> Option<&str> {
    let stem = file_name.split('.').next().unwrap_or(file_name);
    split_hash_suffix(stem).map(|(_, h)| h)
}

/// The extension chain of an artifact file name: everything after the first
/// `.` (`"rlib"`, `"dll.lib"`, `"d"`), or `""` for extension-less files.
pub fn extension_chain(file_name: &str) -> &str {
    match file_name.split_once('.') {
        Some((_, ext)) => ext,
        None => "",
    }
}

/// True for fingerprint-dir *state* files — the ones that name the unit
/// (`lib-serde`, `bin-app`, `run-build-script-build-script-build`) as opposed
/// to metadata (`invoked.timestamp`, `*.json`, `dep-*`, `output-*`).
fn is_state_file(name: &str) -> bool {
    name != "invoked.timestamp"
        && !name.ends_with(".json")
        && !name.starts_with("dep-")
        && !name.starts_with("output-")
}

/// The sorted set of state-file names inside a fingerprint directory — the
/// unit-identity component of the grouping key. A multi-bin package keeps all
/// its `bin-*` fingerprints in ONE dir (spike 0.3 finding 3), so the file SET,
/// not any single file, is the identity.
pub fn unit_state_files<'a, I: IntoIterator<Item = &'a str>>(files: I) -> Vec<String> {
    let mut out: Vec<String> = files
        .into_iter()
        .filter(|f| is_state_file(f))
        .map(str::to_string)
        .collect();
    out.sort();
    out
}

/// Unit kind, derived from the file names inside a fingerprint directory.
/// Used for tier attribution; identity grouping uses [`unit_state_files`]
/// plus the artifact class (spike 0.3: check-mode and build-mode fingerprints
/// of the same unit are distinct live identities).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnitKind {
    Lib,
    Bin,
    Test,
    Example,
    Bench,
    /// Compiling the build script itself (`build-script-*` files).
    BuildScriptCompile,
    /// Running the build script (`run-build-script-*` files).
    BuildScriptRun,
    /// Unrecognized fingerprint contents — kept verbatim so grouping still
    /// separates it; never a deletion candidate on its own.
    Other(String),
}

/// Classify a fingerprint directory from its contained file names.
/// Returns `None` when nothing recognizable is present (⇒ keep, fail-open).
pub fn unit_kind_from_files<'a, I: IntoIterator<Item = &'a str>>(files: I) -> Option<UnitKind> {
    let mut best: Option<UnitKind> = None;
    for f in files {
        if !is_state_file(f) {
            continue;
        }
        let kind = if f.starts_with("run-build-script") {
            UnitKind::BuildScriptRun
        } else if f.starts_with("build-script") {
            UnitKind::BuildScriptCompile
        } else if f.starts_with("lib-") {
            UnitKind::Lib
        } else if f.starts_with("bin-") {
            UnitKind::Bin
        } else if f.starts_with("test-") || f.starts_with("integration-test-") {
            UnitKind::Test
        } else if f.starts_with("example-") {
            UnitKind::Example
        } else if f.starts_with("bench-") {
            UnitKind::Bench
        } else {
            UnitKind::Other(f.to_string())
        };
        // Prefer a specific kind over Other if both appear.
        best = match (best.take(), kind) {
            (Some(UnitKind::Other(_)) | None, k) => Some(k),
            (Some(prev), UnitKind::Other(_)) => Some(prev),
            (Some(prev), _) => Some(prev),
        };
    }
    best
}

/// Parse an `incremental/` child directory name `<crate>-<disambiguator>`
/// into `(crate, disambiguator)`. Grammar per spike 0.4: crate names are
/// sanitized to `[A-Za-z0-9_]`, the disambiguator is lowercase alphanumeric.
/// Returns `None` for anything else (⇒ keep, fail-open).
pub fn incremental_group(dir_name: &str) -> Option<(&str, &str)> {
    let (krate, disambig) = dir_name.rsplit_once('-')?;
    let crate_ok = !krate.is_empty()
        && krate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !krate.contains('-');
    let disambig_ok = !disambig.is_empty()
        && disambig
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (crate_ok && disambig_ok).then_some((krate, disambig))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_grammar() {
        assert!(is_unit_hash("8cf73098e091883d"));
        assert!(!is_unit_hash("8CF73098E091883D")); // uppercase is not Cargo's
        assert!(!is_unit_hash("8cf73098e091883")); // 15 chars
        assert!(!is_unit_hash("8cf73098e091883dz"));
    }

    #[test]
    fn split_and_artifact_hash() {
        assert_eq!(
            split_hash_suffix("serde_core-cff848a14a6a8f25"),
            Some(("serde_core", "cff848a14a6a8f25"))
        );
        assert_eq!(split_hash_suffix("serde_core"), None);
        assert_eq!(
            artifact_hash("libserde-1e3ae50c54bba662.rlib"),
            Some("1e3ae50c54bba662")
        );
        assert_eq!(
            artifact_hash("serde_derive-bd0447a482faacd7.dll.lib"),
            Some("bd0447a482faacd7")
        );
        // uplifted binaries carry no hash — fail-open
        assert_eq!(artifact_hash("spike05_split.exe"), None);
    }

    #[test]
    fn extension_chains() {
        assert_eq!(extension_chain("libserde-1e3ae50c54bba662.rlib"), "rlib");
        assert_eq!(extension_chain("x-1e3ae50c54bba662.dll.lib"), "dll.lib");
        assert_eq!(extension_chain("probe-1e3ae50c54bba662"), "");
    }

    #[test]
    fn state_files_are_sorted_and_filtered() {
        assert_eq!(
            unit_state_files([
                "lib-serde.json",
                "invoked.timestamp",
                "dep-lib-serde",
                "lib-serde",
            ]),
            vec!["lib-serde".to_string()]
        );
        assert_eq!(
            unit_state_files(["bin-b", "bin-a", "bin-a.json"]),
            vec!["bin-a".to_string(), "bin-b".to_string()]
        );
    }

    #[test]
    fn kinds_from_fingerprint_files() {
        assert_eq!(
            unit_kind_from_files([
                "dep-lib-serde",
                "invoked.timestamp",
                "lib-serde",
                "lib-serde.json"
            ]),
            Some(UnitKind::Lib)
        );
        assert_eq!(
            unit_kind_from_files([
                "run-build-script-build-script-build",
                "run-build-script-build-script-build.json"
            ]),
            Some(UnitKind::BuildScriptRun)
        );
        assert_eq!(
            unit_kind_from_files([
                "build-script-build-script-build",
                "build-script-build-script-build.json",
                "dep-build-script-build-script-build",
                "invoked.timestamp"
            ]),
            Some(UnitKind::BuildScriptCompile)
        );
        assert_eq!(
            unit_kind_from_files(["bin-spike05-split", "bin-spike05-split.json"]),
            Some(UnitKind::Bin)
        );
        assert_eq!(unit_kind_from_files(["invoked.timestamp"]), None);
        // Remaining kinds and the Other fallback.
        assert_eq!(unit_kind_from_files(["test-lib-x"]), Some(UnitKind::Test));
        assert_eq!(
            unit_kind_from_files(["integration-test-x"]),
            Some(UnitKind::Test)
        );
        assert_eq!(
            unit_kind_from_files(["example-demo"]),
            Some(UnitKind::Example)
        );
        assert_eq!(unit_kind_from_files(["bench-b"]), Some(UnitKind::Bench));
        assert_eq!(
            unit_kind_from_files(["weird-state-file"]),
            Some(UnitKind::Other("weird-state-file".to_string()))
        );
        // Merge order: a specific kind wins over Other in either order, and
        // the first specific kind is kept.
        assert_eq!(
            unit_kind_from_files(["weird-thing", "lib-x"]),
            Some(UnitKind::Lib)
        );
        assert_eq!(
            unit_kind_from_files(["lib-x", "weird-thing"]),
            Some(UnitKind::Lib)
        );
        assert_eq!(
            unit_kind_from_files(["lib-x", "bin-y"]),
            Some(UnitKind::Lib)
        );
    }

    #[test]
    fn incremental_grammar() {
        assert_eq!(
            incremental_group("spike05_split-1o3vnwkbphdzk"),
            Some(("spike05_split", "1o3vnwkbphdzk"))
        );
        // crate part with a hyphen means the rsplit put it in the crate side —
        // still valid: rsplit takes the LAST '-', crate keeps earlier ones…
        // …but Cargo sanitizes crate names, so a '-' in the crate part is refused.
        assert_eq!(incremental_group("has-hyphen-1o3vnwkbphdzk"), None);
        assert_eq!(incremental_group("noseparator"), None);
        assert_eq!(incremental_group("crate_a-UPPER"), None);
    }
}
