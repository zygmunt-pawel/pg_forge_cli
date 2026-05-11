use std::path::PathBuf;
use thiserror::Error;

pub type Result<T, E = PgForgeError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum PgForgeError {
    #[error("instance {0:?} already exists")]
    InstanceExists(String),

    #[error("instance {0:?} not found")]
    InstanceNotFound(String),

    #[error("invalid instance name {0:?}: must match [a-z][a-z0-9_-]{{0,62}}")]
    InvalidInstanceName(String),

    #[error("no free TCP port in range {start}..{end}")]
    NoFreePort { start: u16, end: u16 },

    #[error("config file at {path:?} is malformed: {source}")]
    ConfigMalformed { path: PathBuf, source: toml::de::Error },

    #[error("docker engine error: {0}")]
    Docker(String),

    #[error("io error at {path:?}: {source}")]
    Io { path: PathBuf, source: std::io::Error },

    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}
