use crate::domain::preset::Preset;
use crate::error::{PgForgeError, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Immutable description of one PG instance managed by pgforge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub db_name: String,
    pub app_user: String,
    pub app_password: String,
    pub pgbackrest_password: String,
    pub preset: Preset,
    pub pg_version: u8,
    pub host_port: u16,
    /// Whether this instance has pgbackrest archiving + S3 backups wired up.
    /// `false` = created with `--no-backup` (local dev / test). On such
    /// instances `pgforge snapshot`, `pgforge clone`, and `pgforge restore`
    /// are refused — there's no archive to read from.
    ///
    /// `#[serde(default)]` keeps state.toml files written before this field
    /// existed loadable (defaults to true = full pgbackrest enabled, which
    /// is what every pre-Plan-4 instance had).
    #[serde(default = "backup_enabled_default")]
    pub backup_enabled: bool,
    /// After `pgforge upgrade`, the data lives in a new docker volume whose
    /// name encodes the post-upgrade version. None = use the convention
    /// `pgforge_data_<name>` (every instance before its first upgrade).
    #[serde(default)]
    pub volume_name_override: Option<String>,
}

fn backup_enabled_default() -> bool {
    true
}

impl Instance {
    /// Effective docker volume name. Honors a post-upgrade override; falls
    /// back to the original `pgforge_data_<name>` convention.
    pub fn volume_name(&self) -> String {
        self.volume_name_override
            .clone()
            .unwrap_or_else(|| format!("pgforge_data_{}", self.name))
    }

    /// Names must be filesystem-safe, DNS-safe, and short enough to fit a
    /// container name. Conservative regex: lowercase start, then
    /// alphanumeric / `_` / `-`, total length 1..=63.
    pub fn validate_name(name: &str) -> Result<()> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9_-]{0,62}$").unwrap());
        if re.is_match(name) {
            Ok(())
        } else {
            Err(PgForgeError::InvalidInstanceName(name.to_string()))
        }
    }
}
