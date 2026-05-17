//! JSON cache of the most recent SMART check. Written once a day by the
//! systemd-user timer's `pgforge smart check --write-cache`; read every
//! 60 s by the TUI poller and pre-dispatch by the CLI banner.
//!
//! All I/O is SYNCHRONOUS by design — the cache file is a few-KB JSON,
//! reads take microseconds, and using `tokio::fs` here would just add
//! task-switch overhead with no latency benefit. The TUI reader poller is
//! aware of this (see `src/tui/refresh.rs::spawn_smart_reader`).

use crate::smart::types::{SmartHealth, SmartUnknownReason};
use std::path::{Path, PathBuf};

pub const STALE_AFTER_HOURS: u32 = 48;

/// `$XDG_STATE_HOME/pgforge/disk-smart.json` (fallback
/// `$HOME/.local/state/pgforge/disk-smart.json`).
pub fn default_cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("pgforge").join("disk-smart.json")
}

/// Best-effort read with explicit failure-reason mapping.
///
/// - File missing → `Unknown(NoCache)`.
/// - File present but unparseable → `Unknown(ParseError)`.
/// - File parses but `checked_at` is older than `max_age_hours` OR in the
///   future (clock skew / NTP step backward) → `Unknown(Stale)`.
/// - Otherwise the deserialized snapshot.
pub fn read_cache(
    path: &Path,
    now: jiff::Timestamp,
    max_age_hours: u32,
) -> SmartHealth {
    let bytes = match std::fs::read(path) {
        Ok(b)  => b,
        Err(_) => return SmartHealth::unknown(SmartUnknownReason::NoCache),
    };
    let h: SmartHealth = match serde_json::from_slice(&bytes) {
        Ok(h)  => h,
        Err(_) => return SmartHealth::unknown(SmartUnknownReason::ParseError),
    };
    if h.is_stale(now, max_age_hours) {
        return SmartHealth::unknown(SmartUnknownReason::Stale);
    }
    h
}

/// Atomic write: tempfile in the SAME parent directory + `persist()`. Same
/// filesystem → rename is atomic. Tempfile mode is 0600 via NamedTempFile.
pub fn write_cache(path: &Path, health: &SmartHealth) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(tmp.as_file_mut(), health)
        .map_err(|e| std::io::Error::other(format!("serialize: {e}")))?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
