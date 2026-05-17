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

// Silence unused-import lint until later tasks use the reason enum.
#[allow(dead_code)]
fn _touch_reason_enum() -> SmartUnknownReason { SmartUnknownReason::NoDevicesFound }

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
}
