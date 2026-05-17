use jiff::Timestamp;
use pgforge::smart::installed::{InstalledState, read_installed, write_installed};
use std::path::PathBuf;

#[test]
fn round_trip() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("smart-installed.json");
    let state = InstalledState {
        smartctl_path: PathBuf::from("/usr/sbin/smartctl"),
        user: "pawel".into(),
        devices: vec![PathBuf::from("/dev/nvme0n1"), PathBuf::from("/dev/sda")],
        installed_at: Timestamp::from_second(1_715_000_000).unwrap(),
    };
    write_installed(&path, &state).unwrap();
    let back = read_installed(&path).unwrap();
    assert_eq!(back.smartctl_path, state.smartctl_path);
    assert_eq!(back.user, state.user);
    assert_eq!(back.devices, state.devices);
    assert_eq!(back.installed_at, state.installed_at);
}

#[test]
fn missing_file_returns_none() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("absent.json");
    assert!(read_installed(&path).is_none());
}

#[test]
fn corrupt_file_returns_none() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("smart-installed.json");
    std::fs::write(&path, b"not json {{{").unwrap();
    assert!(read_installed(&path).is_none());
}
