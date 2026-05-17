use pgforge::cli::format_smart_banner_line;
use pgforge::smart::types::{
    DriveSmart, SmartHealth, SmartStatus, SmartUnknownReason,
};

fn health(status: SmartStatus, device: &str, reasons: &[&str]) -> SmartHealth {
    SmartHealth {
        status,
        worst_device: Some(device.into()),
        worst_reasons: reasons.iter().map(|s| s.to_string()).collect(),
        unknown_reason: None,
        drives: vec![DriveSmart {
            device: device.into(),
            model: "X".into(),
            transport: "nvme".into(),
            status,
            reasons: reasons.iter().map(|s| s.to_string()).collect(),
            unknown_reason: None,
        }],
        checked_at: jiff::Timestamp::from_second(1_715_000_000).unwrap(),
    }
}

#[test]
fn critical_produces_banner_with_device_and_reasons() {
    let h = health(SmartStatus::Critical, "/dev/nvme0n1", &["Reallocated_Sector_Ct=3", "Current_Pending_Sector=1"]);
    let line = format_smart_banner_line(&h).expect("Some line for Critical");
    assert!(line.starts_with("\u{26A0}"));
    assert!(line.contains("SMART CRITICAL"));
    assert!(line.contains("/dev/nvme0n1"));
    assert!(line.contains("Reallocated_Sector_Ct=3"));
    assert!(line.contains("Current_Pending_Sector=1"));
}

#[test]
fn ok_returns_none() {
    let h = health(SmartStatus::Ok, "/dev/nvme0n1", &[]);
    assert!(format_smart_banner_line(&h).is_none());
}

#[test]
fn warn_returns_none() {
    // Warn is shown in TUI only — no CLI banner (anti-desensitization).
    let h = health(SmartStatus::Warn, "/dev/nvme0n1", &["percentage_used=82%"]);
    assert!(format_smart_banner_line(&h).is_none());
}

#[test]
fn unknown_returns_none() {
    let h = SmartHealth::unknown(SmartUnknownReason::NoCache);
    assert!(format_smart_banner_line(&h).is_none());
}
