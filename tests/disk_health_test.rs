use pgforge::disk::health::{DiskHealth, DiskStatus, MountUsage};

#[test]
fn status_threshold_boundaries() {
    assert_eq!(DiskStatus::from_used_pct(0),   DiskStatus::Ok);
    assert_eq!(DiskStatus::from_used_pct(79),  DiskStatus::Ok);
    assert_eq!(DiskStatus::from_used_pct(80),  DiskStatus::Warn);
    assert_eq!(DiskStatus::from_used_pct(89),  DiskStatus::Warn);
    assert_eq!(DiskStatus::from_used_pct(90),  DiskStatus::Critical);
    assert_eq!(DiskStatus::from_used_pct(100), DiskStatus::Critical);
}

#[test]
fn used_pct_rounds_up() {
    // 89.9% used must be reported as 90 (Critical), not 89 (Warn).
    // total=10000, free=11 -> used=9989 -> 9989*100/10000 = 99.89 -> 100
    assert_eq!(MountUsage::compute_pct(10000, 11), 100);
    // total=10000, free=2000 -> used=8000 -> 8000*100/10000 = 80.0 -> 80
    assert_eq!(MountUsage::compute_pct(10000, 2000), 80);
    // total=10000, free=2001 -> used=7999 -> 7999*100/10000 = 79.99 -> 80 (rounds up)
    assert_eq!(MountUsage::compute_pct(10000, 2001), 80);
    // total=10000, free=10000 -> used=0 -> 0
    assert_eq!(MountUsage::compute_pct(10000, 10000), 0);
    // total=0 (empty mount, malformed) -> 0 (don't divide by zero)
    assert_eq!(MountUsage::compute_pct(0, 0), 0);
}

#[test]
fn worst_aggregation() {
    let mounts = vec![
        sample(50, "docker"),
        sample(85, "state"),
        sample(72, "dumps"),
    ];
    let h = DiskHealth::aggregate(mounts);
    assert_eq!(h.status, DiskStatus::Warn);
    assert_eq!(h.worst_pct, 85);
    assert_eq!(h.worst_label, "state");
}

#[test]
fn unknown_when_no_mounts() {
    let h = DiskHealth::aggregate(vec![]);
    assert_eq!(h.status, DiskStatus::Unknown);
    assert_eq!(h.worst_pct, 0);
    assert_eq!(h.worst_label, "");
}

#[test]
fn critical_dominates_warn() {
    let h = DiskHealth::aggregate(vec![
        sample(85, "a"),
        sample(91, "b"),
        sample(50, "c"),
    ]);
    assert_eq!(h.status, DiskStatus::Critical);
    assert_eq!(h.worst_pct, 91);
    assert_eq!(h.worst_label, "b");
}

fn sample(pct: u8, label: &str) -> MountUsage {
    MountUsage {
        mount_label: label.into(),
        mount_path: std::path::PathBuf::from("/"),
        used_pct: pct,
        free_bytes: 0,
        total_bytes: 100,
    }
}
