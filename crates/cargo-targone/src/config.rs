//! Machine configuration: `$CARGO_HOME/targone/config.toml`.
//!
//! ```toml
//! budget = "100GB"                  # optional global cap (F-048)
//! roots = ["E:/HiveLLM", "E:/code"] # scan roots for scheduled runs
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineConfig {
    /// Human size ("20GB", "1.5TiB"); parsed by `targone_core::parse_size`.
    pub budget: Option<String>,
    /// Roots scanned by `schedule run` in addition to registry entries.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
}

impl MachineConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = match std::fs::read_to_string(path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        toml::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn budget_bytes(&self) -> Result<Option<u64>, String> {
        match &self.budget {
            None => Ok(None),
            Some(s) => targone_core::parse_size(s)
                .map(Some)
                .ok_or_else(|| format!("invalid budget size: {s:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default() {
        let t = tempfile::tempdir().unwrap();
        let cfg = MachineConfig::load(&t.path().join("nope.toml")).unwrap();
        assert!(cfg.budget.is_none());
        assert!(cfg.roots.is_empty());
    }

    #[test]
    fn parses_config() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("config.toml");
        std::fs::write(&p, "budget = \"2GiB\"\nroots = [\"/code\"]\n").unwrap();
        let cfg = MachineConfig::load(&p).unwrap();
        assert_eq!(cfg.budget_bytes().unwrap(), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(cfg.roots.len(), 1);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("config.toml");
        std::fs::write(&p, "buget = \"2GiB\"\n").unwrap();
        assert!(MachineConfig::load(&p).is_err());
    }
}
