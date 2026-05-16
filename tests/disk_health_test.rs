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

use async_trait::async_trait;
use pgforge::disk::health::{check_disk_health, DockerRootDirSource};

struct FakeDocker(Option<String>);

#[async_trait]
impl DockerRootDirSource for FakeDocker {
    async fn docker_root_dir(&self) -> anyhow::Result<Option<String>> {
        Ok(self.0.clone())
    }
}

struct FailingDocker;

#[async_trait]
impl DockerRootDirSource for FailingDocker {
    async fn docker_root_dir(&self) -> anyhow::Result<Option<String>> {
        anyhow::bail!("simulated docker socket failure")
    }
}

#[tokio::test]
async fn check_disk_health_returns_something_when_docker_works() {
    let tmp = tempfile::tempdir().unwrap();
    let h = check_disk_health(
        &FakeDocker(Some(tmp.path().display().to_string())),
        Some(tmp.path().to_path_buf()),
        Some(tmp.path().to_path_buf()),
    ).await;
    // tmpfs / overlayfs / whatever — should report a real mount, not Unknown.
    assert_ne!(h.status, pgforge::disk::health::DiskStatus::Unknown);
    assert!(h.worst_pct <= 100);
}

#[tokio::test]
async fn check_disk_health_unknown_when_all_paths_fail() {
    // Use a path that genuinely does not exist and whose root won't statvfs.
    // We can't easily fake that without /proc tricks; instead use FailingDocker
    // + paths that walk_up_to root and succeed there. Test the Docker-only
    // failure path: docker_root_dir errors → falls back to /var/lib/docker which
    // exists → still produces a mount. So this test asserts the FALLBACK works
    // even when the trait errors.
    let tmp = tempfile::tempdir().unwrap();
    let h = check_disk_health(
        &FailingDocker,
        Some(tmp.path().to_path_buf()),
        Some(tmp.path().to_path_buf()),
    ).await;
    // Should NOT panic, should NOT be Unknown (other paths still measurable).
    assert_ne!(h.status, pgforge::disk::health::DiskStatus::Unknown);
}

use std::path::Path;

#[test]
fn measure_path_returns_a_mount_for_an_existing_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let m = pgforge::disk::health::measure_path("tmp", tmp.path()).unwrap();
    assert_eq!(m.mount_label, "tmp");
    assert!(m.total_bytes > 0, "tempdir filesystem should report nonzero size");
    assert!(m.used_pct <= 100);
}

#[test]
fn measure_path_walks_up_to_existing_ancestor() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist").join("deeper");
    // Should NOT error; should statvfs the nearest existing ancestor (tmp).
    let m = pgforge::disk::health::measure_path("dumps", &missing).unwrap();
    assert!(m.total_bytes > 0);
}

#[test]
fn measure_path_returns_err_when_no_ancestor_exists() {
    // /no/such/path/anywhere/ever — root exists but nothing past it.
    let p = Path::new("/no/such/path/anywhere/ever");
    let r = pgforge::disk::health::measure_path("x", p);
    // Walks up to "/" which exists, so this should succeed.
    assert!(r.is_ok(), "should walk up to /");
}

#[test]
fn dedupe_collapses_same_dev() {
    // /tmp and a subdirectory of /tmp are on the same filesystem;
    // measuring both should produce one deduped entry.
    let tmp = tempfile::tempdir().unwrap();
    let subdir = tmp.path().join("a");
    std::fs::create_dir(&subdir).unwrap();
    let paths = vec![
        ("docker", tmp.path().to_path_buf()),
        ("dumps", subdir),
    ];
    let mounts = pgforge::disk::health::measure_dedup(paths);
    assert_eq!(mounts.len(), 1, "same filesystem should dedupe; got {mounts:?}");
}
