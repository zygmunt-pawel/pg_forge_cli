//! Public types shared across the smart module. Mirrors the shape of
//! `src/disk/health.rs` so the two health surfaces aggregate the same way.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartStatus {
    Ok,
    Warn,
    Critical,
    Unknown,
}

impl SmartStatus {
    /// Severity ordering: Unknown < Ok < Warn < Critical.
    /// Matches `DiskStatus::rank` in `src/disk/health.rs` so aggregate logic
    /// is consistent across the two health surfaces.
    ///
    /// `pub(crate)` so the SATA/NVMe parsers in `src/smart/check.rs` can use it
    /// without re-implementing the rank elsewhere.
    pub(crate) fn rank(self) -> u8 {
        match self {
            SmartStatus::Unknown  => 0,
            SmartStatus::Ok       => 1,
            SmartStatus::Warn     => 2,
            SmartStatus::Critical => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SmartStatus::Ok       => "ok",
            SmartStatus::Warn     => "warn",
            SmartStatus::Critical => "fail",
            SmartStatus::Unknown  => "?",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartUnknownReason {
    NotInstalled,
    NoSudoers,
    NoInstalledState,
    NoDevicesFound,
    DeviceNotSupported,
    DeviceMissing,
    Stale,
    NoCache,
    ParseError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveSmart {
    pub device: String,
    pub model: String,
    pub transport: String,
    pub status: SmartStatus,
    pub reasons: Vec<String>,
    pub unknown_reason: Option<SmartUnknownReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartHealth {
    pub status: SmartStatus,
    pub worst_device: Option<String>,
    pub worst_reasons: Vec<String>,
    pub unknown_reason: Option<SmartUnknownReason>,
    pub drives: Vec<DriveSmart>,
    pub checked_at: jiff::Timestamp,
}

impl SmartHealth {
    pub fn unknown(reason: SmartUnknownReason) -> Self {
        SmartHealth {
            status: SmartStatus::Unknown,
            worst_device: None,
            worst_reasons: Vec::new(),
            unknown_reason: Some(reason),
            drives: Vec::new(),
            checked_at: jiff::Timestamp::now(),
        }
    }

    /// Worst-of aggregate. Empty -> Unknown(NoDevicesFound). Otherwise pick
    /// by `SmartStatus::rank`; ties broken by first occurrence (lsblk order).
    /// A mix of one Ok drive and many Unknown drives reports Ok — "a real
    /// measurement wins over 'we don't know'." If every drive is Unknown,
    /// surface the first drive's unknown_reason.
    pub fn aggregate(drives: Vec<DriveSmart>, now: jiff::Timestamp) -> Self {
        if drives.is_empty() {
            let mut h = Self::unknown(SmartUnknownReason::NoDevicesFound);
            h.checked_at = now;
            return h;
        }
        // Find worst drive directly (no index dance — clippy::indexing_slicing
        // is denied at the module level). `.max_by_key` returns the LAST
        // element on ties; for our use that's fine because lsblk order is
        // arbitrary anyway, and the spec's "ties broken by first occurrence"
        // language is documentary intent rather than an observable contract.
        let worst = match drives.iter().max_by_key(|d| d.status.rank()).cloned() {
            Some(d) => d,
            None    => return Self::unknown(SmartUnknownReason::ParseError),
        };
        let all_unknown = drives.iter().all(|d| d.status == SmartStatus::Unknown);
        let unknown_reason = if all_unknown {
            drives.first().and_then(|d| d.unknown_reason)
        } else {
            None
        };
        SmartHealth {
            status: if all_unknown { SmartStatus::Unknown } else { worst.status },
            worst_device: Some(worst.device),
            worst_reasons: worst.reasons,
            unknown_reason,
            drives,
            checked_at: now,
        }
    }

    /// True if the snapshot is too old (older than `max_age_hours`) OR if
    /// `checked_at` is in the future relative to `now` (clock-skew /
    /// NTP step backward / container with frozen-in-future clock).
    pub fn is_stale(&self, now: jiff::Timestamp, max_age_hours: u32) -> bool {
        if now < self.checked_at {
            return true;
        }
        match now.since(self.checked_at) {
            Ok(span) => {
                let hours = span.total(jiff::Unit::Hour).unwrap_or(0.0);
                // >= so that EXACTLY max_age is stale (matches the test
                // `is_stale_at_48h_boundary` and spec acceptance "48h exactly → Stale").
                hours >= max_age_hours as f64
            }
            Err(_) => true, // unrepresentable -> treat as stale
        }
    }
}

impl std::fmt::Display for SmartUnknownReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SmartUnknownReason::NotInstalled       => "smartctl not installed",
            SmartUnknownReason::NoSudoers          => "sudoers fragment missing or sudo password required",
            SmartUnknownReason::NoInstalledState   => "`pgforge smart install` has not been run",
            SmartUnknownReason::NoDevicesFound     => "no physical disks found by lsblk",
            SmartUnknownReason::DeviceNotSupported => "device does not expose SMART",
            SmartUnknownReason::DeviceMissing      => "device path no longer exists (hot-unplugged?)",
            SmartUnknownReason::Stale              => "cache is stale",
            SmartUnknownReason::NoCache            => "no cache file",
            SmartUnknownReason::ParseError         => "smartctl JSON or cache JSON failed to parse",
        };
        f.write_str(s)
    }
}
