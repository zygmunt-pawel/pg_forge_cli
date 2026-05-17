use jiff::{Span, Timestamp};
use pgforge::smart::types::{
    DriveSmart, SmartHealth, SmartStatus, SmartUnknownReason,
};

fn drive(status: SmartStatus, device: &str, reasons: &[&str]) -> DriveSmart {
    DriveSmart {
        device: device.into(),
        model: "X".into(),
        transport: "nvme".into(),
        status,
        reasons: reasons.iter().map(|s| s.to_string()).collect(),
        unknown_reason: if status == SmartStatus::Unknown {
            Some(SmartUnknownReason::DeviceNotSupported)
        } else {
            None
        },
    }
}

fn now() -> Timestamp { Timestamp::from_second(1_715_000_000).unwrap() }

#[test]
fn empty_aggregates_to_no_devices_found() {
    let h = SmartHealth::aggregate(vec![], now());
    assert_eq!(h.status, SmartStatus::Unknown);
    assert_eq!(h.unknown_reason, Some(SmartUnknownReason::NoDevicesFound));
    assert_eq!(h.worst_device, None);
    assert!(h.worst_reasons.is_empty());
}

#[test]
fn single_ok_aggregates_to_ok() {
    let h = SmartHealth::aggregate(vec![drive(SmartStatus::Ok, "/dev/nvme0n1", &[])], now());
    assert_eq!(h.status, SmartStatus::Ok);
    assert_eq!(h.worst_device.as_deref(), Some("/dev/nvme0n1"));
}

#[test]
fn critical_dominates_warn_and_ok() {
    let h = SmartHealth::aggregate(vec![
        drive(SmartStatus::Ok,       "/dev/sda",     &[]),
        drive(SmartStatus::Warn,     "/dev/sdb",     &["Temperature=62"]),
        drive(SmartStatus::Critical, "/dev/nvme0n1", &["Reallocated_Sector_Ct=3"]),
    ], now());
    assert_eq!(h.status, SmartStatus::Critical);
    assert_eq!(h.worst_device.as_deref(), Some("/dev/nvme0n1"));
    assert_eq!(h.worst_reasons, vec!["Reallocated_Sector_Ct=3".to_string()]);
}

#[test]
fn ok_wins_over_unknown() {
    // "A real measurement wins over 'we don't know'." Documented aggregate
    // semantics — do not change without re-reading the spec.
    let h = SmartHealth::aggregate(vec![
        drive(SmartStatus::Unknown, "/dev/vda", &[]),
        drive(SmartStatus::Ok,      "/dev/sda", &[]),
    ], now());
    assert_eq!(h.status, SmartStatus::Ok);
    assert_eq!(h.worst_device.as_deref(), Some("/dev/sda"));
}

#[test]
fn all_unknown_carries_first_reason() {
    let mut a = drive(SmartStatus::Unknown, "/dev/vda", &[]);
    a.unknown_reason = Some(SmartUnknownReason::DeviceNotSupported);
    let mut b = drive(SmartStatus::Unknown, "/dev/vdb", &[]);
    b.unknown_reason = Some(SmartUnknownReason::ParseError);
    let h = SmartHealth::aggregate(vec![a, b], now());
    assert_eq!(h.status, SmartStatus::Unknown);
    assert_eq!(h.unknown_reason, Some(SmartUnknownReason::DeviceNotSupported));
}

#[test]
fn is_stale_at_48h_boundary() {
    let mut h = SmartHealth::aggregate(vec![drive(SmartStatus::Ok, "/dev/sda", &[])], now());
    h.checked_at = now().checked_sub(Span::new().hours(48)).unwrap();
    assert!(h.is_stale(now(), 48));
}

#[test]
fn is_not_stale_at_47h59m() {
    let mut h = SmartHealth::aggregate(vec![drive(SmartStatus::Ok, "/dev/sda", &[])], now());
    h.checked_at = now()
        .checked_sub(Span::new().hours(47).minutes(59))
        .unwrap();
    assert!(!h.is_stale(now(), 48));
}

#[test]
fn future_checked_at_is_stale() {
    // Clock-skew fail-safe: a checked_at in the future means we can't trust
    // the snapshot (NTP step backward, container with frozen clock, etc.).
    let mut h = SmartHealth::aggregate(vec![drive(SmartStatus::Ok, "/dev/sda", &[])], now());
    h.checked_at = now().checked_add(Span::new().minutes(10)).unwrap();
    assert!(h.is_stale(now(), 48));
}

#[test]
fn status_label_strings() {
    assert_eq!(SmartStatus::Ok.label(),       "ok");
    assert_eq!(SmartStatus::Warn.label(),     "warn");
    assert_eq!(SmartStatus::Critical.label(), "fail");
    assert_eq!(SmartStatus::Unknown.label(),  "?");
}
