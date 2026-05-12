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
    spawn_snapshots(tx, names_rx, state_root);
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
                let list = match snapshots::run(&n, Some(root.clone())) {
                    Ok(l) => l, Err(_) => Vec::new(),
                };
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
