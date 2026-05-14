use crate::domain::instance::Instance;
use crate::error::{PgForgeError, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceState {
    pub instance: Instance,
    pub created_at: String, // ISO-8601, kept as String to avoid pulling chrono yet
}

impl InstanceState {
    pub fn default_state_root() -> PathBuf {
        ProjectDirs::from("dev", "pgforge", "pgforge")
            .map(|p| p.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn dir_under(state_root: &Path, name: &str) -> PathBuf {
        state_root.join("instances").join(name)
    }

    fn file_under(state_root: &Path, name: &str) -> PathBuf {
        Self::dir_under(state_root, name).join("state.toml")
    }

    pub fn save_under(&self, state_root: &Path) -> Result<()> {
        Instance::validate_name(&self.instance.name)?;
        let dir = Self::dir_under(state_root, &self.instance.name);
        // 0700 — state.toml contains plaintext app_password + pgbackrest_password.
        crate::util::fs::create_secret_dir(&dir)?;
        let file = Self::file_under(state_root, &self.instance.name);
        let raw = toml::to_string_pretty(self).map_err(|e| {
            PgForgeError::Anyhow(anyhow::anyhow!("serialize instance state: {e}"))
        })?;
        crate::util::fs::write_secret(&file, raw)
    }

    pub fn load_under(state_root: &Path, name: &str) -> Result<Self> {
        Instance::validate_name(name)?;
        let file = Self::file_under(state_root, name);
        if !file.exists() {
            return Err(PgForgeError::InstanceNotFound(name.to_string()));
        }
        let raw = std::fs::read_to_string(&file).map_err(|e| PgForgeError::Io {
            path: file.clone(),
            source: e,
        })?;
        toml::from_str(&raw).map_err(|source| PgForgeError::ConfigMalformed {
            path: file,
            source,
        })
    }

    pub fn list_under(state_root: &Path) -> Result<Vec<String>> {
        let dir = state_root.join("instances");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&dir).map_err(|e| PgForgeError::Io {
            path: dir.clone(),
            source: e,
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| PgForgeError::Io {
                path: dir.clone(),
                source: e,
            })?;
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
        Ok(out)
    }

    pub fn exists_under(state_root: &Path, name: &str) -> bool {
        Self::file_under(state_root, name).exists()
    }

    /// Safe load-modify-write primitive. Acquires the state-root lock,
    /// re-loads the instance from disk, applies `f`, and atomically saves the
    /// result — all under the lock. Re-loading inside the lock is what
    /// prevents the lost-update race: a concurrent writer (the launchd
    /// snapshot tick, the TUI, another CLI invocation) can't have its update
    /// silently clobbered.
    ///
    /// The lock is held only for the fast file I/O — never pass a closure
    /// that does slow work (docker, network).
    pub fn update_under<F>(state_root: &Path, name: &str, f: F) -> Result<Self>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        let _lock = crate::util::fs::LockedStateRoot::acquire(state_root)?;
        let mut state = Self::load_under(state_root, name)?;
        f(&mut state)?;
        state.save_under(state_root)?;
        Ok(state)
    }

    /// Like `save_under`, but holds the state-root lock for the write. Use
    /// when persisting a freshly-built instance (create/clone/restore) so the
    /// save can't interleave with another writer.
    pub fn save_under_locked(&self, state_root: &Path) -> Result<()> {
        let _lock = crate::util::fs::LockedStateRoot::acquire(state_root)?;
        self.save_under(state_root)
    }
}
