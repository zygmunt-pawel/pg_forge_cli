use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::S3Settings;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GlobalConfig {
    pub s3: Option<S3Settings>,
    pub port_range_start: u16,
    pub port_range_end: u16,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            s3: None,
            port_range_start: 5433,
            port_range_end: 5500,
        }
    }
}

impl GlobalConfig {
    /// Resolve the canonical platform-specific path for the global config.
    pub fn default_path() -> PathBuf {
        ProjectDirs::from("dev", "pgforge", "pgforge")
            .map(|p| p.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::default_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).map_err(|e| PgForgeError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        toml::from_str(&raw).map_err(|source| PgForgeError::ConfigMalformed {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::default_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| PgForgeError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let raw = toml::to_string_pretty(self).map_err(|e| {
            PgForgeError::Anyhow(anyhow::anyhow!("serialize global config: {e}"))
        })?;
        std::fs::write(path, raw).map_err(|e| PgForgeError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }
}
