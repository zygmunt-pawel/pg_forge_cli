use crate::domain::snapshot::SnapshotRecord;
use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapshotsFile {
    pub snapshots: Vec<SnapshotRecord>,
}

impl SnapshotsFile {
    fn file_path(state_root: &Path, instance_name: &str) -> std::path::PathBuf {
        state_root
            .join("instances")
            .join(instance_name)
            .join("snapshots.toml")
    }

    pub fn load_for(state_root: &Path, instance_name: &str) -> Result<Self> {
        crate::domain::instance::Instance::validate_name(instance_name)?;
        let path = Self::file_path(state_root, instance_name);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| PgForgeError::Io {
            path: path.clone(),
            source: e,
        })?;
        toml::from_str(&raw).map_err(|source| PgForgeError::ConfigMalformed {
            path,
            source,
        })
    }

    pub fn save_for(&self, state_root: &Path, instance_name: &str) -> Result<()> {
        crate::domain::instance::Instance::validate_name(instance_name)?;
        if !InstanceState::exists_under(state_root, instance_name) {
            return Err(PgForgeError::InstanceNotFound(instance_name.to_string()));
        }
        let path = Self::file_path(state_root, instance_name);
        let raw = toml::to_string_pretty(self).map_err(|e| {
            PgForgeError::Anyhow(anyhow::anyhow!("serialize snapshots.toml: {e}"))
        })?;
        std::fs::write(&path, raw).map_err(|e| PgForgeError::Io { path, source: e })
    }
}
