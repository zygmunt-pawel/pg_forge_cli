#![allow(clippy::indexing_slicing)]

use pgforge::smart::check::parse_smartctl_json;
use pgforge::smart::types::{SmartStatus, SmartUnknownReason};

fn load(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/smart").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"))
}

#[test]
fn sata_ok() {
    let d = parse_smartctl_json(&load("sata_ok.json"));
    assert_eq!(d.status, SmartStatus::Ok);
    assert_eq!(d.transport, "ATA");
    assert!(d.reasons.is_empty());
}

#[test]
fn sata_reallocated_is_critical() {
    let d = parse_smartctl_json(&load("sata_reallocated_3.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("Reallocated_Sector_Ct=3")));
}

#[test]
fn sata_pending_is_critical() {
    let d = parse_smartctl_json(&load("sata_pending_1.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("Current_Pending_Sector=1")));
}

#[test]
fn sata_offline_uncorrectable_is_critical() {
    let d = parse_smartctl_json(&load("sata_offline_uncorrectable_1.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("Offline_Uncorrectable=1")));
}

#[test]
fn sata_temp_65_is_warn() {
    let d = parse_smartctl_json(&load("sata_temp_65.json"));
    assert_eq!(d.status, SmartStatus::Warn);
    assert!(d.reasons.iter().any(|r| r.contains("Temperature=65")));
}

#[test]
fn sata_temp_55_is_ok() {
    let d = parse_smartctl_json(&load("sata_temp_55.json"));
    assert_eq!(d.status, SmartStatus::Ok);
}

#[test]
fn sas_attached_sata_dispatches_to_sata_parser() {
    // device.protocol = "ATA" even though lsblk reports tran=sas; the parser
    // must trust device.protocol, not whatever was in the discovery call.
    let d = parse_smartctl_json(&load("sas_attached_sata_ok.json"));
    assert_eq!(d.status, SmartStatus::Ok);
    assert_eq!(d.transport, "ATA");
}

#[test]
fn nvme_ok() {
    let d = parse_smartctl_json(&load("nvme_ok.json"));
    assert_eq!(d.status, SmartStatus::Ok);
    assert_eq!(d.transport, "NVMe");
}

#[test]
fn nvme_critical_warning_spare() {
    let d = parse_smartctl_json(&load("nvme_critical_warning_spare.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("available_spare_below_threshold")));
}

#[test]
fn nvme_critical_warning_temp() {
    // bit 1 of critical_warning = temperature_above_threshold
    let d = parse_smartctl_json(&load("nvme_critical_warning_temp.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("temperature_above_threshold")));
}

#[test]
fn nvme_media_errors_is_critical() {
    let d = parse_smartctl_json(&load("nvme_media_errors_5.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("media_errors=5")));
}

#[test]
fn nvme_spare_below_threshold_is_critical() {
    let d = parse_smartctl_json(&load("nvme_spare_below_threshold.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("available_spare=5") && r.contains("threshold=10")));
}

#[test]
fn nvme_percentage_used_82_is_warn() {
    let d = parse_smartctl_json(&load("nvme_percentage_used_82.json"));
    assert_eq!(d.status, SmartStatus::Warn);
    assert!(d.reasons.iter().any(|r| r.contains("percentage_used=82")));
}

#[test]
fn nvme_temp_75_celsius_is_warn() {
    let d = parse_smartctl_json(&load("nvme_temp_75_celsius.json"));
    assert_eq!(d.status, SmartStatus::Warn);
    assert!(d.reasons.iter().any(|r| r.contains("Temperature=75")));
}

#[test]
fn nvme_temp_kelvin_fallback_is_warn() {
    // smartmontools omits top-level temperature.current; parse_nvme must
    // fall back to nvme_smart_health_information_log.temperature (kelvin)
    // and convert. 348 K = 75 °C → Warn.
    let d = parse_smartctl_json(&load("nvme_temp_only_kelvin.json"));
    assert_eq!(d.status, SmartStatus::Warn);
    assert!(d.reasons.iter().any(|r| r.contains("Temperature=75")));
}

#[test]
fn empty_bytes_is_parse_error() {
    let d = parse_smartctl_json(b"");
    assert_eq!(d.status, SmartStatus::Unknown);
    assert_eq!(d.unknown_reason, Some(SmartUnknownReason::ParseError));
}

#[test]
fn unknown_protocol_is_parse_error() {
    let json = br#"{"device":{"protocol":"SAT"},"smart_status":{"passed":true}}"#;
    let d = parse_smartctl_json(json);
    assert_eq!(d.status, SmartStatus::Unknown);
    assert_eq!(d.unknown_reason, Some(SmartUnknownReason::ParseError));
}
