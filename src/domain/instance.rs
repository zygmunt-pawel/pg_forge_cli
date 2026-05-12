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
    /// pgbackrest retention window in days. After each new full backup,
    /// pgbackrest deletes any full older than `retain_days` along with
    /// the WAL needed to recover from them — so R2 storage stays bounded
    /// and PITR window slides forward. 0 = keep everything (no expiry).
    /// Default 30 days = roughly RDS Standard retention.
    #[serde(default = "retain_days_default")]
    pub retain_days: u32,
    /// Hour of day (0..=23, LOCAL time) at which `pgforge schedule`
    /// should auto-snapshot this instance. None = manual only.
    /// Default Some(3) = 03:00 local (typical low-load window).
    #[serde(default = "snapshot_hour_default")]
    pub snapshot_hour: Option<u8>,
    /// RFC3339 timestamp of the last snapshot (auto or manual). Used by
    /// `pgforge snapshot --due` to decide whether today's window has
    /// already been satisfied. None for instances that never had a
    /// snapshot yet.
    #[serde(default)]
    pub last_snapshot_at: Option<String>,
}

fn backup_enabled_default() -> bool {
    true
}

fn retain_days_default() -> u32 {
    30
}

fn snapshot_hour_default() -> Option<u8> {
    Some(3)
}

impl Instance {
    /// Effective docker volume name. Honors a post-upgrade override; falls
    /// back to the original `pgforge_data_<name>` convention.
    pub fn volume_name(&self) -> String {
        self.volume_name_override
            .clone()
            .unwrap_or_else(|| format!("pgforge_data_{}", self.name))
    }

    /// Snapshot hour must be in 0..=23 (a valid hour of the day).
    /// A value like 24 or 99 causes `is_snapshot_due` to never fire,
    /// silently killing the scheduler for that instance.
    pub fn validate_snapshot_hour(h: u8) -> Result<()> {
        if h > 23 {
            return Err(PgForgeError::Anyhow(anyhow::anyhow!(
                "snapshot_hour must be 0..=23, got {h}"
            )));
        }
        Ok(())
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
