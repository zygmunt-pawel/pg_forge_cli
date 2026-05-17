//! Record of what `pgforge smart install` set up — used at runtime by
//! `run_smartctl` to know which absolute `smartctl` binary path the sudoers
//! rule grants, and by `pgforge smart status` to surface install age.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledState {
    pub smartctl_path: PathBuf,
    pub user: String,
    pub devices: Vec<PathBuf>,
    pub installed_at: jiff::Timestamp,
}

/// Default path under XDG_STATE_HOME. Falls back to $HOME/.local/state.
pub fn default_installed_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("pgforge").join("smart-installed.json")
}

/// Best-effort read. Missing or corrupt → None (caller treats as
/// `SmartUnknownReason::NoInstalledState`).
pub fn read_installed(path: &Path) -> Option<InstalledState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Atomic write via tempfile + persist (same-fs rename).
pub fn write_installed(path: &Path, state: &InstalledState) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(p) => p,
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no parent",
            ))
        }
    };
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(tmp.as_file_mut(), state)
        .map_err(|e| std::io::Error::other(format!("serialize: {e}")))?;
    match tmp.persist(path) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.error),
    }
}
