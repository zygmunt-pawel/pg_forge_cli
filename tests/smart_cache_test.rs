use jiff::{Span, Timestamp};
use pgforge::smart::cache::{STALE_AFTER_HOURS, read_cache, write_cache};
use pgforge::smart::types::{
    DriveSmart, SmartHealth, SmartStatus, SmartUnknownReason,
};

fn now() -> Timestamp { Timestamp::from_second(1_715_000_000).unwrap() }

fn sample_health(checked_at: Timestamp) -> SmartHealth {
    let drive = DriveSmart {
        device: "/dev/nvme0n1".into(),
        model: "X".into(),
        transport: "nvme".into(),
        status: SmartStatus::Ok,
        reasons: vec![],
        unknown_reason: None,
    };
    let mut h = SmartHealth::aggregate(vec![drive], checked_at);
    h.checked_at = checked_at;
    h
}

#[test]
fn round_trip_fresh() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    let h = sample_health(now());
    write_cache(&path, &h).unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Ok);
    assert_eq!(back.worst_device.as_deref(), Some("/dev/nvme0n1"));
    assert_eq!(back.unknown_reason, None);
}

#[test]
fn missing_file_returns_no_cache() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("absent.json");
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Unknown);
    assert_eq!(back.unknown_reason, Some(SmartUnknownReason::NoCache));
}

#[test]
fn corrupt_json_returns_parse_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    std::fs::write(&path, b"not json {{{").unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Unknown);
    assert_eq!(back.unknown_reason, Some(SmartUnknownReason::ParseError));
}

#[test]
fn boundary_48h_is_stale() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    let checked = now().checked_sub(Span::new().hours(48)).unwrap();
    write_cache(&path, &sample_health(checked)).unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Unknown);
    assert_eq!(back.unknown_reason, Some(SmartUnknownReason::Stale));
}

#[test]
fn just_under_48h_is_fresh() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    let checked = now().checked_sub(Span::new().hours(47).minutes(59)).unwrap();
    write_cache(&path, &sample_health(checked)).unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Ok);
    assert_eq!(back.unknown_reason, None);
}

#[test]
fn future_checked_at_is_stale() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    let checked = now().checked_add(Span::new().minutes(10)).unwrap();
    write_cache(&path, &sample_health(checked)).unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Unknown);
    assert_eq!(back.unknown_reason, Some(SmartUnknownReason::Stale));
}
