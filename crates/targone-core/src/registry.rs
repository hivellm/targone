//! The machine-global project registry: an append-only JSONL file recording
//! every workspace that announced itself (via `cargo targone scan` today, the
//! `targone` beacon crate in phase 4).
//!
//! Append-only by design (F-059/F-017): writers only ever add one line —
//! cheap, lock-free, safe from concurrent build scripts. Compaction happens
//! at read time, in memory. The record survives the project going dormant,
//! which is exactly when it is most valuable.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Line {
    v: u32,
    root: PathBuf,
    ts: u64,
}

/// One workspace as seen through the registry (compacted view).
#[derive(Debug, Clone, Serialize)]
pub struct RegistryEntry {
    pub root: PathBuf,
    pub first_seen: u64,
    pub last_seen: u64,
}

impl RegistryEntry {
    /// The workspace no longer exists on disk — its target dirs are orphans
    /// eligible for full reclaim (upstream #13136's model).
    pub fn is_orphan(&self) -> bool {
        !self.root.exists()
    }
}

#[derive(Debug, Clone)]
pub struct Registry {
    path: PathBuf,
}

impl Registry {
    pub fn open(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Append one sighting of `root`. Never rewrites existing content.
    pub fn record(&self, root: &Path) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let line = Line {
            v: 1,
            root: root.to_path_buf(),
            ts: Self::now(),
        };
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let json = serde_json::to_string(&line).map_err(io::Error::other)?;
        writeln!(f, "{json}")
    }

    /// Compacted view: one entry per root, first/last sighting. Unreadable
    /// lines are skipped (a torn append must never poison the registry).
    pub fn entries(&self) -> io::Result<Vec<RegistryEntry>> {
        let content = match fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut map: BTreeMap<PathBuf, (u64, u64)> = BTreeMap::new();
        for raw in content.lines() {
            let Ok(line) = serde_json::from_str::<Line>(raw) else {
                continue;
            };
            let e = map.entry(line.root).or_insert((line.ts, line.ts));
            e.0 = e.0.min(line.ts);
            e.1 = e.1.max(line.ts);
        }
        Ok(map
            .into_iter()
            .map(|(root, (first_seen, last_seen))| RegistryEntry {
                root,
                first_seen,
                last_seen,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_compact() {
        let t = tempfile::tempdir().unwrap();
        let reg = Registry::open(t.path().join("targone/registry.jsonl"));
        let proj = t.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        reg.record(&proj).unwrap();
        reg.record(&proj).unwrap();
        reg.record(&t.path().join("gone-project")).unwrap();
        let entries = reg.entries().unwrap();
        assert_eq!(entries.len(), 2);
        let live = entries.iter().find(|e| e.root == proj).unwrap();
        assert!(!live.is_orphan());
        assert!(live.last_seen >= live.first_seen);
        let gone = entries.iter().find(|e| e.root != proj).unwrap();
        assert!(gone.is_orphan());
    }

    #[test]
    fn torn_lines_are_skipped() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("registry.jsonl");
        fs::write(&path, "{\"v\":1,\"root\":\"/a\",\"ts\":5}\ngarbage{{{\n").unwrap();
        let reg = Registry::open(path);
        assert_eq!(reg.entries().unwrap().len(), 1);
    }

    #[test]
    fn missing_file_is_empty() {
        let t = tempfile::tempdir().unwrap();
        let reg = Registry::open(t.path().join("nope.jsonl"));
        assert!(reg.entries().unwrap().is_empty());
    }
}
