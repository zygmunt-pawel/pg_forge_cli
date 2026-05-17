//! SMART check pipeline: discover physical disks via lsblk → run smartctl
//! per disk (sudo) → parse JSON → aggregate. The whole pipeline is
//! best-effort: anything that fails maps to a `DriveSmart` with
//! `SmartStatus::Unknown` and a specific `SmartUnknownReason`.

use crate::smart::types::SmartUnknownReason;
use serde::Deserialize;
use std::path::PathBuf;

/// One physical disk discovered on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDisk {
    pub path: PathBuf,        // "/dev/nvme0n1"
    pub transport: String,    // "sata" | "sas" | "nvme"
    pub model: String,        // best-effort model string from lsblk
}

#[derive(Debug, Deserialize)]
struct LsblkRoot {
    #[serde(default)]
    blockdevices: Vec<LsblkDevice>,
}

#[derive(Debug, Deserialize)]
struct LsblkDevice {
    name: String,
    #[serde(default, rename = "type")]
    dev_type: Option<String>,
    #[serde(default)]
    tran: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// Parse `lsblk -d -o NAME,TYPE,TRAN,MODEL -J` JSON output into the
/// discovered-disk list. Filters: type=="disk" AND tran in {sata,sas,nvme}.
/// Tolerant of missing optional fields; any device whose JSON entry fails
/// to deserialize is silently skipped (lsblk -J had quoting bugs through
/// util-linux 2.38).
pub fn parse_lsblk_json(json: &[u8]) -> Vec<DiscoveredDisk> {
    let root: LsblkRoot = match serde_json::from_slice(json) {
        Ok(r)  => r,
        Err(_) => return Vec::new(),
    };
    root.blockdevices
        .into_iter()
        .filter_map(|d| {
            let dt = d.dev_type.as_deref()?;
            if dt != "disk" { return None; }
            let tran = d.tran?;
            if !matches!(tran.as_str(), "sata" | "sas" | "nvme") {
                return None;
            }
            Some(DiscoveredDisk {
                path: PathBuf::from(format!("/dev/{}", d.name)),
                transport: tran,
                model: d.model.unwrap_or_default(),
            })
        })
        .collect()
}

/// Discover physical disks by invoking `lsblk -d -o NAME,TYPE,TRAN,MODEL -J`.
/// Failure (lsblk missing, exit non-zero, JSON broken) → empty Vec; caller
/// degrades to `SmartUnknownReason::NoDevicesFound`.
pub async fn discover_disks() -> Vec<DiscoveredDisk> {
    let out = tokio::process::Command::new("lsblk")
        .args(["-d", "-o", "NAME,TYPE,TRAN,MODEL", "-J"])
        .output()
        .await;
    let stdout = match out {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => {
            tracing::warn!(target: "pgforge::smart",
                "lsblk exit {}: {}", o.status,
                String::from_utf8_lossy(&o.stderr));
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(target: "pgforge::smart", "lsblk spawn failed: {e}");
            return Vec::new();
        }
    };
    parse_lsblk_json(&stdout)
}

/// Whether to run sudo non-interactively (`-n`, used by the timer where we
/// can't prompt) or interactively (used by `pgforge smart check` from a
/// TTY without --write-cache, where the user is sitting at the keyboard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SudoMode {
    NonInteractive,
    Interactive,
}

/// Spawn sudo + smartctl on one device, return stdout bytes on success or
/// a classified `SmartUnknownReason` on failure. 5-second per-call timeout.
pub async fn run_smartctl(
    smartctl_path: &std::path::Path,
    device: &std::path::Path,
    mode: SudoMode,
) -> Result<Vec<u8>, SmartUnknownReason> {
    let mut cmd = tokio::process::Command::new("sudo");
    if mode == SudoMode::NonInteractive {
        cmd.arg("-n");
    }
    cmd.arg(smartctl_path)
        .args(["-H", "-A", "-j"])
        .arg(device);

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cmd.output(),
    ).await {
        Ok(Ok(o))  => o,
        Ok(Err(e)) => {
            tracing::warn!(target: "pgforge::smart",
                "spawn sudo smartctl {device:?}: {e}");
            return Err(SmartUnknownReason::NotInstalled);
        }
        Err(_) => {
            tracing::warn!(target: "pgforge::smart",
                "smartctl {device:?} timed out after 5s");
            return Err(SmartUnknownReason::ParseError);
        }
    };

    // Non-zero exit code may still produce valid JSON (smartctl uses its
    // exit code as a bitfield; OVERALL_HEALTH=FAILED returns nonzero but
    // emits JSON). Trust stdout if it parses; only fall back to stderr
    // classification when stdout is empty.
    if !output.stdout.is_empty() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(classify_smartctl_failure(&stderr))
}

/// Map a smartctl / sudo stderr blob to the most specific
/// `SmartUnknownReason`. Order matters — match the more specific patterns
/// first.
pub fn classify_smartctl_failure(stderr: &str) -> SmartUnknownReason {
    if stderr.contains("a password is required") {
        return SmartUnknownReason::NoSudoers;
    }
    if stderr.contains("command not found") {
        return SmartUnknownReason::NotInstalled;
    }
    if stderr.contains("No such file or directory") {
        return SmartUnknownReason::DeviceMissing;
    }
    if stderr.contains("Unknown USB bridge")
        || stderr.contains("does not support SMART")
    {
        return SmartUnknownReason::DeviceNotSupported;
    }
    SmartUnknownReason::ParseError
}

use crate::smart::types::{DriveSmart, SmartStatus};

pub const SATA_TEMP_WARN_C:   i64 = 60;
pub const NVME_TEMP_WARN_C:   i64 = 70;
pub const NVME_WEAR_WARN_PCT: u32 = 80;

/// Top-level structural decoder. We only deserialize what we actually need;
/// `#[serde(default)]` on every nested field so the parser tolerates older /
/// newer smartmontools schema variations.
#[derive(Deserialize)]
struct SmartctlJson {
    #[serde(default)]
    device: SmartctlDevice,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    smart_status: SmartctlSmartStatus,
    #[serde(default)]
    temperature: Option<SmartctlTemperature>,
    #[serde(default)]
    ata_smart_attributes: Option<AtaSmartAttributes>,
    #[serde(default)]
    nvme_smart_health_information_log: Option<NvmeSmartLog>,
}

#[derive(Default, Deserialize)]
struct SmartctlDevice {
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Default, Deserialize)]
struct SmartctlSmartStatus {
    #[serde(default)]
    passed: Option<bool>,
}

#[derive(Deserialize)]
struct SmartctlTemperature {
    #[serde(default)]
    current: Option<i64>,
}

#[derive(Deserialize)]
struct AtaSmartAttributes {
    #[serde(default)]
    table: Vec<AtaAttribute>,
}

#[derive(Deserialize)]
struct AtaAttribute {
    id: u8,
    #[serde(default)]
    name: Option<String>,
    raw: AtaRaw,
}

#[derive(Deserialize)]
struct AtaRaw {
    value: i64,
}

#[derive(Deserialize)]
struct NvmeSmartLog {
    #[serde(default)] critical_warning:          Option<u32>,
    #[serde(default)] available_spare:           Option<u32>,
    #[serde(default)] available_spare_threshold: Option<u32>,
    #[serde(default)] percentage_used:           Option<u32>,
    #[serde(default)] media_errors:              Option<u64>,
    #[serde(default)] temperature:               Option<i64>, // kelvin (older smartmontools)
}

/// Parse one device's `smartctl -H -A -j` JSON output. Dispatches on
/// `device.protocol` (NOT the lsblk transport — SAS-attached SATA reports
/// protocol="ATA" even though lsblk called the transport "sas"). Any failure
/// produces a `DriveSmart` with `SmartStatus::Unknown` and a specific
/// `SmartUnknownReason`.
pub fn parse_smartctl_json(json: &[u8]) -> DriveSmart {
    let parsed: SmartctlJson = match serde_json::from_slice(json) {
        Ok(p)  => p,
        Err(_) => return unknown_drive("?", "", SmartUnknownReason::ParseError, "json parse failed"),
    };
    let device = parsed.device.name.clone().unwrap_or_else(|| "?".to_string());
    let model  = parsed.model_name.clone().unwrap_or_default();
    let protocol = parsed.device.protocol.clone().unwrap_or_default();

    match protocol.as_str() {
        // SAS-native drives also come through as protocol="SCSI" — the
        // parser silently returns Ok with no reasons because parse_sata
        // only iterates ata_smart_attributes.table (which SAS-native does
        // not have). This is a spec-documented best-effort limitation;
        // covering real SAS error counters is queued separately.
        "ATA" | "SCSI" => parse_sata(&parsed, &device, &model, &protocol),
        "NVMe"         => parse_nvme(&parsed, &device, &model, &protocol),
        other => unknown_drive(
            &device, "",  // empty transport (NOT the reason name — that ends up rendered as a label)
            SmartUnknownReason::ParseError,
            &format!("unsupported device.protocol={other}"),
        ),
    }
}

fn unknown_drive(device: &str, transport: &str, reason: SmartUnknownReason, msg: &str) -> DriveSmart {
    DriveSmart {
        device: device.to_string(),
        model: String::new(),
        transport: transport.to_string(),
        status: SmartStatus::Unknown,
        reasons: vec![msg.to_string()],
        unknown_reason: Some(reason),
    }
}

fn parse_sata(p: &SmartctlJson, device: &str, model: &str, transport: &str) -> DriveSmart {
    let mut reasons: Vec<String> = Vec::new();
    let mut status = SmartStatus::Ok;
    let bump = |s: SmartStatus, r: String, status: &mut SmartStatus, reasons: &mut Vec<String>| {
        if s.rank() > status.rank() {
            *status = s;
        }
        reasons.push(r);
    };

    if p.smart_status.passed == Some(false) {
        bump(SmartStatus::Critical, "OVERALL_HEALTH=FAILED".to_string(), &mut status, &mut reasons);
    }

    let table: &[AtaAttribute] = p
        .ata_smart_attributes
        .as_ref()
        .map(|a| a.table.as_slice())
        .unwrap_or(&[]);
    for attr in table {
        let name = attr.name.clone().unwrap_or_default();
        match attr.id {
            5  if attr.raw.value > 0 => bump(
                SmartStatus::Critical,
                format!("Reallocated_Sector_Ct={}", attr.raw.value),
                &mut status, &mut reasons,
            ),
            197 if attr.raw.value > 0 => bump(
                SmartStatus::Critical,
                format!("Current_Pending_Sector={}", attr.raw.value),
                &mut status, &mut reasons,
            ),
            198 if attr.raw.value > 0 => bump(
                SmartStatus::Critical,
                format!("Offline_Uncorrectable={}", attr.raw.value),
                &mut status, &mut reasons,
            ),
            190 | 194 if attr.raw.value > SATA_TEMP_WARN_C => bump(
                SmartStatus::Warn,
                format!("Temperature={}", attr.raw.value),
                &mut status, &mut reasons,
            ),
            _ => { let _ = name; }
        }
    }

    DriveSmart {
        device: device.to_string(),
        model: model.to_string(),
        transport: transport.to_string(),
        status,
        reasons,
        unknown_reason: None,
    }
}

/// Decode a NVMe critical_warning bitmap into human-readable bit names per the
/// NVMe spec. Empty Vec when no bits are set.
fn decode_nvme_critical_warning(bits: u32) -> Vec<String> {
    let names = [
        (0, "available_spare_below_threshold"),
        (1, "temperature_above_threshold"),
        (2, "nvm_reliability_degraded"),
        (3, "media_read_only"),
        (4, "volatile_memory_backup_failed"),
        (5, "persistent_memory_region_unreliable"),
    ];
    names.iter()
        .filter(|(bit, _)| (bits >> bit) & 1 == 1)
        .map(|(_, name)| (*name).to_string())
        .collect()
}

fn parse_nvme(p: &SmartctlJson, device: &str, model: &str, transport: &str) -> DriveSmart {
    let mut reasons: Vec<String> = Vec::new();
    let mut status = SmartStatus::Ok;
    let bump = |s: SmartStatus, r: String, status: &mut SmartStatus, reasons: &mut Vec<String>| {
        if s.rank() > status.rank() {
            *status = s;
        }
        reasons.push(r);
    };

    if p.smart_status.passed == Some(false) {
        bump(SmartStatus::Critical, "OVERALL_HEALTH=FAILED".to_string(), &mut status, &mut reasons);
    }

    let log = match &p.nvme_smart_health_information_log {
        Some(l) => l,
        None => {
            return unknown_drive(
                device, transport,
                SmartUnknownReason::ParseError,
                "missing nvme_smart_health_information_log",
            );
        }
    };

    if let Some(cw) = log.critical_warning && cw != 0 {
        let decoded = decode_nvme_critical_warning(cw);
        let joined = if decoded.is_empty() {
            format!("critical_warning={cw}")
        } else {
            format!("critical_warning: {}", decoded.join(","))
        };
        bump(SmartStatus::Critical, joined, &mut status, &mut reasons);
    }
    if let Some(me) = log.media_errors && me > 0 {
        bump(SmartStatus::Critical, format!("media_errors={me}"), &mut status, &mut reasons);
    }
    if let (Some(spare), Some(thr)) = (log.available_spare, log.available_spare_threshold)
        && spare < thr
    {
        bump(
            SmartStatus::Critical,
            format!("available_spare={spare}% < threshold={thr}%"),
            &mut status, &mut reasons,
        );
    }
    if let Some(pct) = log.percentage_used && pct >= NVME_WEAR_WARN_PCT {
        bump(SmartStatus::Warn, format!("percentage_used={pct}%"), &mut status, &mut reasons);
    }

    // Prefer the normalised top-level Celsius. Fall back to kelvin in
    // nvme_smart_health_information_log.temperature (older smartmontools).
    let temp_c: Option<i64> = p.temperature.as_ref().and_then(|t| t.current)
        .or_else(|| log.temperature.map(|k| k - 273));
    if let Some(c) = temp_c && c > NVME_TEMP_WARN_C {
        bump(SmartStatus::Warn, format!("Temperature={c}"), &mut status, &mut reasons);
    }

    DriveSmart {
        device: device.to_string(),
        model: model.to_string(),
        transport: transport.to_string(),
        status,
        reasons,
        unknown_reason: None,
    }
}

use crate::smart::installed::InstalledState;
use crate::smart::types::SmartHealth;

/// Wire discover → run_smartctl → parse → aggregate. Returns a fully
/// populated `SmartHealth` ready to write to the cache. Never fails — every
/// per-device error produces a `DriveSmart` with `SmartStatus::Unknown`.
///
/// `installed` is the persisted record from `pgforge smart install` (used
/// for the smartctl absolute path). If None → degrade every drive to
/// `Unknown(NoInstalledState)`.
pub async fn check_all(
    installed: Option<&InstalledState>,
    sudo_mode: SudoMode,
) -> SmartHealth {
    let now = jiff::Timestamp::now();
    let discovered = discover_disks().await;
    if discovered.is_empty() {
        let mut h = SmartHealth::unknown(SmartUnknownReason::NoDevicesFound);
        h.checked_at = now;
        return h;
    }
    let Some(state) = installed else {
        let drives = discovered.into_iter().map(|d| DriveSmart {
            device: d.path.display().to_string(),
            model: d.model,
            transport: d.transport,
            status: SmartStatus::Unknown,
            reasons: vec!["pgforge smart install has not been run".to_string()],
            unknown_reason: Some(SmartUnknownReason::NoInstalledState),
        }).collect::<Vec<_>>();
        return SmartHealth::aggregate(drives, now);
    };

    let mut drives: Vec<DriveSmart> = Vec::with_capacity(discovered.len());
    for disk in discovered {
        let result = run_smartctl(&state.smartctl_path, &disk.path, sudo_mode).await;
        let drive = match result {
            Ok(bytes) => {
                let mut d = parse_smartctl_json(&bytes);
                // Preserve the lsblk-known model if smartctl didn't echo one.
                if d.model.is_empty() { d.model = disk.model.clone(); }
                // Always overwrite device with the canonical lsblk path
                // (smartctl sometimes echoes a different alias).
                d.device = disk.path.display().to_string();
                d
            }
            Err(reason) => DriveSmart {
                device: disk.path.display().to_string(),
                model: disk.model,
                transport: disk.transport,
                status: SmartStatus::Unknown,
                reasons: vec![format!("{reason}")], // SmartUnknownReason: Display (added in T1)
                unknown_reason: Some(reason),
            },
        };
        drives.push(drive);
    }
    SmartHealth::aggregate(drives, now)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn filters_to_physical_disks() {
        let json = br#"{
            "blockdevices": [
                {"name":"nvme0n1","type":"disk","tran":"nvme","model":"SK hynix"},
                {"name":"sda","type":"disk","tran":"sata","model":"Samsung 870"},
                {"name":"loop0","type":"loop","tran":null,"model":null},
                {"name":"dm-0","type":"crypt","tran":null,"model":null},
                {"name":"sdb1","type":"part","tran":null,"model":null},
                {"name":"vda","type":"disk","tran":"virtio","model":""}
            ]
        }"#;
        let disks = parse_lsblk_json(json);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].path, std::path::PathBuf::from("/dev/nvme0n1"));
        assert_eq!(disks[0].transport, "nvme");
        assert_eq!(disks[1].path, std::path::PathBuf::from("/dev/sda"));
        assert_eq!(disks[1].transport, "sata");
    }

    #[test]
    fn empty_on_no_blockdevices() {
        let json = br#"{"blockdevices":[]}"#;
        assert!(parse_lsblk_json(json).is_empty());
    }

    #[test]
    fn empty_on_garbage_json() {
        assert!(parse_lsblk_json(b"not json").is_empty());
    }

    #[test]
    fn skips_devices_missing_required_fields() {
        let json = br#"{
            "blockdevices":[
                {"name":"sda"},
                {"name":"sdb","type":"disk","tran":"sata","model":"X"}
            ]
        }"#;
        let disks = parse_lsblk_json(json);
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].path, std::path::PathBuf::from("/dev/sdb"));
    }

    #[test]
    fn classify_smartctl_stderr_no_such_file() {
        let stderr = "Smartctl open device: /dev/sda failed: No such file or directory";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::DeviceMissing,
        );
    }

    #[test]
    fn classify_smartctl_stderr_not_supported() {
        let stderr = "Device does not support SMART";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::DeviceNotSupported,
        );
    }

    #[test]
    fn classify_smartctl_stderr_unknown_usb_bridge() {
        let stderr = "Smartctl open device: /dev/sdc failed: Unknown USB bridge";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::DeviceNotSupported,
        );
    }

    #[test]
    fn classify_sudo_password_required() {
        let stderr = "sudo: a password is required";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::NoSudoers,
        );
    }

    #[test]
    fn classify_command_not_found() {
        let stderr = "sudo: smartctl: command not found";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::NotInstalled,
        );
    }

    #[test]
    fn classify_anything_else_is_parse_error() {
        let stderr = "weird unexpected message";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::ParseError,
        );
    }
}
