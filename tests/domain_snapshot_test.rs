use pgforge::domain::instance::Instance;
use pgforge::domain::snapshot::{SnapshotKind, SnapshotRecord};

#[test]
fn snapshot_record_round_trips_via_toml() {
    let rec = SnapshotRecord {
        label: "20260511-141259F".into(),
        kind: SnapshotKind::Full,
        user_label: Some("before-migration".into()),
        taken_at: "2026-05-11T14:12:59Z".into(),
    };
    let s = toml::to_string(&rec).unwrap();
    let back: SnapshotRecord = toml::from_str(&s).unwrap();
    assert_eq!(rec, back);
}

#[test]
fn snapshot_kind_serializes_lowercase() {
    // TOML requires a table at the root, so we wrap kind in a minimal struct.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Wrapper {
        kind: SnapshotKind,
    }
    let s = toml::to_string(&Wrapper { kind: SnapshotKind::Full }).unwrap();
    assert!(s.contains("full"), "got: {s}");
    let s = toml::to_string(&Wrapper { kind: SnapshotKind::Diff }).unwrap();
    assert!(s.contains("diff"));
}

#[test]
fn snapshot_hour_rejects_24() {
    assert!(Instance::validate_snapshot_hour(24).is_err());
    assert!(Instance::validate_snapshot_hour(99).is_err());
}

#[test]
fn snapshot_hour_accepts_full_day_range() {
    for h in 0..=23u8 {
        assert!(Instance::validate_snapshot_hour(h).is_ok(), "hour {h}");
    }
}
