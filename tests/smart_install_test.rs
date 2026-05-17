use pgforge::smart::install::{
    InstallError, render_service_unit, render_sudoers_fragment, render_timer_unit,
};
use std::path::PathBuf;

#[test]
fn sudoers_happy_path() {
    let out = render_sudoers_fragment(
        "pawel",
        std::path::Path::new("/usr/sbin/smartctl"),
        &[PathBuf::from("/dev/nvme0n1"), PathBuf::from("/dev/sda")],
    ).expect("render");
    assert!(out.contains("# pgforge SMART disk health checks"));
    assert!(out.contains("pawel ALL=(root) NOPASSWD: /usr/sbin/smartctl -H -A -j /dev/nvme0n1"));
    assert!(out.contains("pawel ALL=(root) NOPASSWD: /usr/sbin/smartctl -H -A -j /dev/sda"));
    // One rule per line — count newlines beginning with the user prefix.
    let count = out.lines().filter(|l| l.starts_with("pawel ")).count();
    assert_eq!(count, 2);
}

#[test]
fn sudoers_empty_devices_is_err() {
    let result = render_sudoers_fragment(
        "pawel",
        std::path::Path::new("/usr/sbin/smartctl"),
        &[],
    );
    assert!(matches!(result, Err(InstallError::NoDevices)));
}

#[test]
fn timer_unit_has_daily_persistent_randomized() {
    let unit = render_timer_unit();
    assert!(unit.contains("OnCalendar=daily"));
    assert!(unit.contains("RandomizedDelaySec=1h"));
    assert!(unit.contains("Persistent=true"));
    assert!(unit.contains("Unit=pgforge-smart.service"));
    assert!(unit.contains("WantedBy=timers.target"));
}

#[test]
fn service_unit_uses_absolute_pgforge_path() {
    let unit = render_service_unit(std::path::Path::new("/home/pawel/.local/bin/pgforge"));
    assert!(unit.contains("Type=oneshot"));
    assert!(unit.contains("ExecStart=/home/pawel/.local/bin/pgforge smart check --write-cache"));
}
