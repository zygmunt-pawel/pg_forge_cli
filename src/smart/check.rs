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
