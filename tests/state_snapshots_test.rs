use pgforge::domain::snapshot::{SnapshotKind, SnapshotRecord};
use pgforge::state::snapshots::SnapshotsFile;
use tempfile::TempDir;

fn rec(label: &str) -> SnapshotRecord {
    SnapshotRecord {
        label: label.into(),
        kind: SnapshotKind::Full,
        user_label: None,
        taken_at: "2026-05-11T14:00:00Z".into(),
    }
}

#[test]
fn load_returns_empty_when_file_missing() {
    let dir = TempDir::new().unwrap();
    let file = SnapshotsFile::load_for(dir.path(), "billing").unwrap();
    assert!(file.snapshots.is_empty());
}

#[test]
fn append_and_load_round_trip() {
    let dir = TempDir::new().unwrap();
    // SnapshotsFile::save_for requires that the instance exists (validated
    // via InstanceState::exists_under). For this test we manually plant a
    // dummy state.toml so save_for accepts the call.
    let instance_dir = dir.path().join("instances").join("billing");
    std::fs::create_dir_all(&instance_dir).unwrap();
    std::fs::write(instance_dir.join("state.toml"), "").unwrap();

    let mut file = SnapshotsFile::load_for(dir.path(), "billing").unwrap();
    file.snapshots.push(rec("20260511-A"));
    file.snapshots.push(rec("20260511-B"));
    file.save_for(dir.path(), "billing").unwrap();

    let loaded = SnapshotsFile::load_for(dir.path(), "billing").unwrap();
    assert_eq!(loaded.snapshots.len(), 2);
    assert_eq!(loaded.snapshots[1].label, "20260511-B");
}

#[test]
fn malformed_snapshots_file_returns_typed_error() {
    use pgforge::error::PgForgeError;
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path().join("instances").join("billing");
    std::fs::create_dir_all(&dir_path).unwrap();
    std::fs::write(dir_path.join("snapshots.toml"), "garbage [[[").unwrap();
    let err = SnapshotsFile::load_for(dir.path(), "billing").unwrap_err();
    assert!(matches!(err, PgForgeError::ConfigMalformed { .. }));
}
