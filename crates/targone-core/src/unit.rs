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
pub fn artifact_hash(file_name: &str) -> Option<&str> {
    let stem = file_name.split('.').next().unwrap_or(file_name);
    split_hash_suffix(stem).map(|(_, h)| h)
}

/// Unit kind, derived from the file names inside a fingerprint directory.
/// Distinct kinds of the same package are distinct units with distinct hashes
/// (the "package shows up twice" property), so recency grouping must key on
/// `(package, kind)` — never on package alone.
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
        // Skip metadata files: the kind marker is the bare state file.
        if f == "invoked.timestamp" || f.ends_with(".json") {
            continue;
        }
        if f.starts_with("dep-") || f.starts_with("output-") {
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
