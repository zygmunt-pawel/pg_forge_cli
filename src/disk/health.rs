//! Host disk monitoring. Best-effort: never panics, never propagates errors
//! to the caller. Status is one of Ok / Warn / Critical / Unknown — Unknown
//! is distinct from Ok and means "we could not measure", surfaced as `Disk ?`
//! in the TUI, no banner in the CLI.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskStatus {
    Ok,
    Warn,
    Critical,
    Unknown,
}

impl DiskStatus {
    /// 0..=79 → Ok, 80..=89 → Warn, 90..=100 → Critical.
    /// (Pre-clamped percentages only; caller guarantees 0..=100.)
    pub fn from_used_pct(pct: u8) -> Self {
        match pct {
            0..=79  => DiskStatus::Ok,
            80..=89 => DiskStatus::Warn,
            _       => DiskStatus::Critical,
        }
    }

    /// Severity ordering: Unknown < Ok < Warn < Critical.
    /// Unknown is the lowest because "we don't know" should never override
    /// a real measurement.
    fn rank(self) -> u8 {
        match self {
            DiskStatus::Unknown  => 0,
            DiskStatus::Ok       => 1,
            DiskStatus::Warn     => 2,
            DiskStatus::Critical => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MountUsage {
    pub mount_label: String,
    pub mount_path: PathBuf,
    pub used_pct: u8,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

impl MountUsage {
    /// Used % rounded UP so 89.9% becomes 90 (Critical), never 89 (Warn).
    /// total=0 → 0 (avoid div-by-zero on degenerate inputs).
    pub fn compute_pct(total_bytes: u64, free_bytes: u64) -> u8 {
        if total_bytes == 0 {
            return 0;
        }
        let used = total_bytes.saturating_sub(free_bytes);
        let pct = (used.saturating_mul(100)).div_ceil(total_bytes);
        pct.min(100) as u8
    }
}

#[derive(Debug, Clone)]
pub struct DiskHealth {
    pub status: DiskStatus,
    pub worst_pct: u8,
    pub worst_label: String,
    pub worst_mount: PathBuf,
}

impl DiskHealth {
    pub fn unknown() -> Self {
        DiskHealth {
            status: DiskStatus::Unknown,
            worst_pct: 0,
            worst_label: String::new(),
            worst_mount: PathBuf::new(),
        }
    }

    /// Reduce per-mount measurements to one aggregate health snapshot.
    /// Empty input → Unknown (distinct from "all Ok"); else picks the
    /// mount with the highest severity (ties broken by highest pct).
    pub fn aggregate(mounts: Vec<MountUsage>) -> Self {
        if mounts.is_empty() {
            return Self::unknown();
        }
        let mut worst: Option<&MountUsage> = None;
        let mut worst_status = DiskStatus::Ok;
        for m in &mounts {
            let s = DiskStatus::from_used_pct(m.used_pct);
            let take = match worst {
                None => true,
                Some(w) => {
                    s.rank() > DiskStatus::from_used_pct(w.used_pct).rank()
                        || (s.rank() == DiskStatus::from_used_pct(w.used_pct).rank()
                            && m.used_pct > w.used_pct)
                }
            };
            if take {
                worst = Some(m);
                worst_status = s;
            }
        }
        // worst is Some because mounts.is_empty() is false; pattern-match it
        // without unwrap (banned by file-level clippy lint).
        let w = match worst {
            Some(w) => w,
            None    => return Self::unknown(),
        };
        DiskHealth {
            status: worst_status,
            worst_pct: w.used_pct,
            worst_label: w.mount_label.clone(),
            worst_mount: w.mount_path.clone(),
        }
    }
}
