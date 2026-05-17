//! Background pollers — ls (5s), status (2s), snapshots+pitr (15s).
//! Each is its own tokio::task spawned at TUI startup. They consume
//! the latest instance-name list via a watch::Receiver and emit
//! refresh events into the main channel.

use crate::commands::{ls, snapshots, status};
use crate::docker::bollard_engine::BollardEngine;
use crate::state::instance::InstanceState;
use crate::tui::events::{Event, SnapshotsView};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc::UnboundedSender, watch};

const LS_PERIOD: Duration = Duration::from_secs(5);
const STATUS_PERIOD: Duration = Duration::from_secs(2);
const SNAP_PERIOD: Duration = Duration::from_secs(15);

pub fn spawn_pollers(
    tx: UnboundedSender<Event>,
    names_rx: watch::Receiver<Vec<String>>,
    state_root: Option<PathBuf>,
) {
    spawn_ls(tx.clone(), state_root.clone());
    spawn_status(tx.clone(), names_rx.clone(), state_root.clone());
    spawn_snapshots(tx.clone(), names_rx, state_root);
    // Disk-health poller wraps the same BollardEngine the other pollers
    // create per-tick. Cheaper to share one connect() here.
    if let Ok(docker) = crate::docker::bollard_engine::BollardEngine::connect() {
        spawn_disk_health(std::sync::Arc::new(docker), tx.clone());
    } else {
        tracing::warn!(target: "pgforge::tui::refresh",
            "disk-health poller: BollardEngine::connect failed; status will be Unknown");
    }
    spawn_smart_reader(tx, crate::smart::cache::default_cache_path());
}

fn spawn_ls(tx: UnboundedSender<Event>, state_root: Option<PathBuf>) {
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(LS_PERIOD);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            iv.tick().await;
            let r = tokio::time::timeout(
                Duration::from_secs(5),
                ls::run(ls::LsArgs { override_state_root: state_root.clone() }),
            ).await;
            match r {
                Ok(Ok(rows)) => { let _ = tx.send(Event::InstancesListed(rows)); }
                Ok(Err(e))   => { tracing::warn!(target: "pgforge::tui::refresh", "ls poller: {e}"); }
                Err(_)       => { tracing::warn!(target: "pgforge::tui::refresh", "ls poller: docker call timed out after 5s; marking stale"); }
            }
        }
    });
}

fn spawn_status(
    tx: UnboundedSender<Event>,
    mut names_rx: watch::Receiver<Vec<String>>,
    state_root: Option<PathBuf>,
) {
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(STATUS_PERIOD);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            iv.tick().await;
            let names = names_rx.borrow_and_update().clone();
            let docker = match BollardEngine::connect() {
                Ok(d) => d,
                Err(e) => {
                    for n in &names {
                        let _ = tx.send(Event::RefreshFailed { name: n.clone(), err: e.to_string() });
                    }
                    continue;
                }
            };
            let root = state_root.clone().unwrap_or_else(InstanceState::default_state_root);
            for n in names {
                let r = tokio::time::timeout(
                    Duration::from_secs(5),
                    status::run_with_engine(
                        status::StatusArgs { name: n.clone(), override_state_root: Some(root.clone()) },
                        &docker, root.clone(),
                    ),
                ).await;
                match r {
                    Ok(Ok(s))  => { let _ = tx.send(Event::StatusRefreshed { name: n, status: s }); }
                    Ok(Err(e)) => { let _ = tx.send(Event::RefreshFailed { name: n, err: e.to_string() }); }
                    Err(_)     => {
                        tracing::warn!(target: "pgforge::tui::refresh",
                            "status poller: docker call timed out after 5s for {n}; marking stale");
                        let _ = tx.send(Event::RefreshFailed { name: n, err: "docker call timed out".to_string() });
                    }
                }
            }
        }
    });
}

fn spawn_snapshots(
    tx: UnboundedSender<Event>,
    mut names_rx: watch::Receiver<Vec<String>>,
    state_root: Option<PathBuf>,
) {
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(SNAP_PERIOD);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            iv.tick().await;
            let names = names_rx.borrow_and_update().clone();
            let docker = match BollardEngine::connect() { Ok(d) => d, Err(_) => continue };
            let root = state_root.clone().unwrap_or_else(InstanceState::default_state_root);
            for n in names {
                let list = snapshots::run(&n, Some(root.clone())).unwrap_or_default();
                let pitr = match tokio::time::timeout(
                    Duration::from_secs(5),
                    snapshots::pitr_window(&n, &docker, &root),
                ).await {
                    Ok(Ok(w))  => w,
                    Ok(Err(_)) => Default::default(),
                    Err(_)     => {
                        tracing::warn!(target: "pgforge::tui::refresh",
                            "snapshots poller: docker call timed out after 5s for {n}; marking stale");
                        Default::default()
                    }
                };
                let _ = tx.send(Event::SnapshotsRefreshed { name: n, view: SnapshotsView { list, pitr } });
            }
        }
    });
}

const DISK_PERIOD: Duration = Duration::from_secs(15);
const DISK_TIMEOUT: Duration = Duration::from_secs(2);

/// Periodic disk-health poller. Bounded per-tick by DISK_TIMEOUT to
/// guard against a hung NFS / FUSE mount freezing the poller forever.
/// On timeout / error → emits DiskHealth::unknown() so the TUI can
/// show "Disk ?" instead of going stale.
pub fn spawn_disk_health<D>(
    docker: std::sync::Arc<D>,
    tx: UnboundedSender<Event>,
) -> tokio::task::JoinHandle<()>
where
    D: crate::disk::health::DockerRootDirSource + 'static,
{
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(DISK_PERIOD);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            iv.tick().await;
            let h = match tokio::time::timeout(
                DISK_TIMEOUT,
                crate::disk::health::check_disk_health(&*docker, None, None),
            ).await {
                Ok(h)  => h,
                Err(_) => {
                    tracing::warn!(target: "pgforge::tui::refresh",
                        "disk-health poll timed out after {:?}", DISK_TIMEOUT);
                    crate::disk::health::DiskHealth::unknown()
                }
            };
            let _ = tx.send(Event::DiskHealthRefreshed(h));
        }
    })
}

const SMART_READ_PERIOD: std::time::Duration = std::time::Duration::from_secs(60);

/// 60-second poller that reads the SMART cache file (no smartctl invocation,
/// no sudo, no Docker call — pure file read). Eager first read so the TUI
/// footer doesn't show `SMART ?` for a full minute on startup when there's
/// a valid cache sitting on disk.
pub fn spawn_smart_reader(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    cache_path: std::path::PathBuf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Eager first read.
        let h = crate::smart::cache::read_cache(
            &cache_path,
            jiff::Timestamp::now(),
            crate::smart::cache::STALE_AFTER_HOURS,
        );
        let _ = tx.send(Event::SmartRefreshed(h));

        let mut iv = tokio::time::interval(SMART_READ_PERIOD);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The first iv.tick() fires immediately — consume it since we
        // already did the eager read above.
        iv.tick().await;
        loop {
            iv.tick().await;
            let h = crate::smart::cache::read_cache(
                &cache_path,
                jiff::Timestamp::now(),
                crate::smart::cache::STALE_AFTER_HOURS,
            );
            let _ = tx.send(Event::SmartRefreshed(h));
        }
    })
}

/// On-demand refresh of a single instance — used after a successful op
/// to surface the new state without waiting a poller tick.
pub fn refresh_one(name: String, tx: UnboundedSender<Event>, state_root: Option<PathBuf>) {
    tokio::spawn(async move {
        let docker = match BollardEngine::connect() { Ok(d) => d, Err(_) => return };
        let root = state_root.clone().unwrap_or_else(InstanceState::default_state_root);
        if let Ok(s) = status::run_with_engine(
            status::StatusArgs { name: name.clone(), override_state_root: Some(root.clone()) },
            &docker, root.clone(),
        ).await {
            let _ = tx.send(Event::StatusRefreshed { name: name.clone(), status: s });
        }
        let list = snapshots::run(&name, Some(root.clone())).unwrap_or_default();
        let pitr = snapshots::pitr_window(&name, &docker, &root).await.unwrap_or_default();
        let _ = tx.send(Event::SnapshotsRefreshed { name, view: SnapshotsView { list, pitr } });
    });
}

#[cfg(test)]
mod smart_reader_tests {
    use super::*;
    use crate::smart::cache::write_cache;
    use crate::smart::types::{DriveSmart, SmartHealth, SmartStatus};
    use tokio::sync::mpsc::unbounded_channel;

    #[tokio::test]
    async fn spawn_smart_reader_emits_event_eagerly() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("disk-smart.json");
        let drive = DriveSmart {
            device: "/dev/nvme0n1".into(),
            model: "T".into(),
            transport: "nvme".into(),
            status: SmartStatus::Ok,
            reasons: vec![],
            unknown_reason: None,
        };
        let health = SmartHealth::aggregate(vec![drive], jiff::Timestamp::now());
        write_cache(&path, &health).unwrap();
        let (tx, mut rx) = unbounded_channel();
        let h = spawn_smart_reader(tx, path);
        let ev = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await.expect("event in 3s").expect("channel open");
        assert!(matches!(ev, Event::SmartRefreshed(_)));
        h.abort();
    }
}

#[cfg(test)]
mod disk_health_poller_tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[tokio::test]
    async fn spawn_disk_health_emits_event_within_two_ticks() {
        // Use a sentinel implementation that returns Unknown immediately
        // so the test doesn't depend on a Docker daemon.
        struct InstantDocker;
        #[async_trait::async_trait]
        impl crate::disk::health::DockerRootDirSource for InstantDocker {
            async fn docker_root_dir(&self) -> anyhow::Result<Option<String>> {
                Ok(None)
            }
        }
        let (tx, mut rx) = unbounded_channel();
        let docker = std::sync::Arc::new(InstantDocker);
        let h = spawn_disk_health(docker, tx);
        // First tick fires immediately on interval start, so we should see
        // an event in well under the 15s poll period.
        let ev = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            rx.recv(),
        ).await.expect("event in 3s").expect("channel open");
        assert!(matches!(ev, Event::DiskHealthRefreshed(_)));
        h.abort();
    }
}
