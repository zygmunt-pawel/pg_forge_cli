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
}

impl Instance {
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
