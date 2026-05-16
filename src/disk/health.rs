//! Host disk monitoring. Best-effort: never panics, never propagates errors
//! to the caller. Status is one of Ok / Warn / Critical / Unknown — Unknown
//! is distinct from Ok and means "we could not measure", surfaced as `Disk ?`
//! in the TUI, no banner in the CLI.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

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

/// Best-effort statvfs of `path`, walking up to the first existing ancestor
/// (so a not-yet-created `~/pgforge-dumps` falls back to `$HOME`). Returns
/// Err on any failure; callers drop the mount silently.
pub fn measure_path(label: &str, path: &Path) -> Result<MountUsage, std::io::Error> {
    let existing = walk_up_to_existing(path)?;
    let stat = nix::sys::statvfs::statvfs(&existing)
        .map_err(|e| std::io::Error::other(format!("statvfs: {e}")))?;
    let frsize = stat.fragment_size() as u64;
    let total_bytes = stat.blocks() as u64 * frsize;
    let free_bytes = stat.blocks_available() as u64 * frsize;
    let used_pct = MountUsage::compute_pct(total_bytes, free_bytes);
    Ok(MountUsage {
        mount_label: label.to_string(),
        mount_path: existing,
        used_pct,
        free_bytes,
        total_bytes,
    })
}

fn walk_up_to_existing(start: &Path) -> Result<PathBuf, std::io::Error> {
    let mut p: &Path = start;
    loop {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        match p.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => p = parent,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("no ancestor of {start:?} exists"),
                ))
            }
        }
    }
}

use async_trait::async_trait;

#[async_trait]
pub trait DockerRootDirSource: Send + Sync {
    async fn docker_root_dir(&self) -> anyhow::Result<Option<String>>;
}

/// Aggregate disk-health across the three filesystems pgforge actually
/// uses: Docker volumes (host-side), pgforge state dir, pgforge dumps
/// dir. Best-effort: any sub-failure drops that mount silently; all
/// failures → Unknown.
///
/// `state_root` and `dumps_root` are accepted as Option for testability;
/// in production the caller passes None and we resolve via the standard
/// XDG paths.
pub async fn check_disk_health<D: DockerRootDirSource>(
    docker: &D,
    state_root: Option<PathBuf>,
    dumps_root: Option<PathBuf>,
) -> DiskHealth {
    let docker_root = docker
        .docker_root_dir()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "/var/lib/docker".to_string());
    let state = state_root.unwrap_or_else(
        crate::state::instance::InstanceState::default_state_root,
    );
    let dumps = match dumps_root {
        Some(p) => p,
        None => crate::commands::dump::default_dump_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp")),
    };

    let paths = vec![
        ("docker", PathBuf::from(docker_root)),
        ("state", state),
        ("dumps", dumps),
    ];
    DiskHealth::aggregate(measure_dedup(paths))
}

/// Measure each labelled path and drop duplicates by device id
/// (st_dev). The first labelled occurrence wins.
pub fn measure_dedup(paths: Vec<(&str, PathBuf)>) -> Vec<MountUsage> {
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut out: Vec<MountUsage> = Vec::new();
    for (label, p) in paths {
        match measure_path(label, &p) {
            Ok(m) => {
                let dev = match m.mount_path.metadata() {
                    Ok(md) => md.dev(),
                    Err(e) => {
                        tracing::warn!(
                            target: "pgforge::disk",
                            "metadata({path:?}) failed: {e}; dropping mount",
                            path = m.mount_path
                        );
                        continue;
                    }
                };
                if seen.insert(dev) {
                    out.push(m);
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "pgforge::disk",
                    "measure {label} {p:?} failed: {e}; dropping mount"
                );
            }
        }
    }
    out
}
