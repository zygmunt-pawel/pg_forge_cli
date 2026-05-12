//! Long-op runner — wraps commands::{snapshot,clone,rotate,upgrade,restore}
//! and emits OpStarted / OpFinished into the TUI event channel. Decodes
//! the string-encoded args from AppState::pending_ops.

use crate::error::Result;
use crate::tui::events::{Event, OpKind};
use std::path::PathBuf;
use std::pin::Pin;
use tokio::sync::mpsc::UnboundedSender;

type BoxFut = Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>;

pub fn spawn(
    kind: OpKind,
    encoded: String,
    tx: UnboundedSender<Event>,
    state_root: Option<PathBuf>,
) {
    tokio::spawn(async move {
        // Identify the user-visible instance (the one whose lock will be
        // taken). For Clone/Restore that's the source; for Snapshot/
        // Rotate/Upgrade it's the only name in the encoded string.
        let (display_name, fut): (String, BoxFut) = match kind {
            OpKind::Snapshot => (encoded.clone(), Box::pin(run_snapshot(encoded, state_root))),
            OpKind::Rotate   => (encoded.clone(), Box::pin(run_rotate(encoded, state_root))),
            OpKind::Upgrade  => {
                let (name, to) = parse_at(&encoded);
                (name.clone(), Box::pin(run_upgrade(name, to, state_root)))
            }
            OpKind::Clone => {
                let (src, as_) = parse_colon(&encoded);
                (src.clone(), Box::pin(run_clone(src, as_, state_root)))
            }
            OpKind::Restore => {
                let (src, as_, tt) = parse_colon_at(&encoded);
                (src.clone(), Box::pin(run_restore(src, as_, tt, state_root)))
            }
            OpKind::Clipboard => {
                // Clipboard is sync and never spawned; this branch is
                // unreachable in practice but must be exhaustive.
                return;
            }
        };
        let _ = tx.send(Event::OpStarted { instance: display_name.clone(), kind });
        let result = fut.await.map_err(|e| e.to_string());
        let _ = tx.send(Event::OpFinished { instance: display_name, kind, result });
    });
}

fn parse_colon(s: &str) -> (String, String) {
    let mut it = s.splitn(2, ':');
    (it.next().unwrap().to_string(), it.next().unwrap_or("").to_string())
}
fn parse_at(s: &str) -> (String, u8) {
    let mut it = s.splitn(2, '@');
    let name = it.next().unwrap().to_string();
    let to: u8 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (name, to)
}
fn parse_colon_at(s: &str) -> (String, String, Option<String>) {
    let (left, right) = parse_colon(s);
    if let Some(idx) = right.find('@') {
        let (as_, t) = right.split_at(idx);
        (left, as_.to_string(), Some(t[1..].to_string()))
    } else {
        (left, right, None)
    }
}

async fn run_snapshot(instance: String, state_root: Option<PathBuf>) -> Result<()> {
    use crate::commands::snapshot::{run, SnapshotArgs};
    run(SnapshotArgs { instance, user_label: None, override_state_root: state_root }).await?;
    Ok(())
}

async fn run_clone(source: String, as_name: String, state_root: Option<PathBuf>) -> Result<()> {
    use crate::commands::clone::{run, CloneArgs};
    run(CloneArgs { source, as_name, override_state_root: state_root }).await?;
    Ok(())
}

async fn run_rotate(name: String, state_root: Option<PathBuf>) -> Result<()> {
    use crate::commands::rotate::{run, RotateArgs};
    run(RotateArgs { name, override_state_root: state_root }).await?;
    Ok(())
}

async fn run_upgrade(name: String, to_version: u8, state_root: Option<PathBuf>) -> Result<()> {
    use crate::commands::upgrade::{run, UpgradeArgs};
    run(UpgradeArgs { name, to_version, override_state_root: state_root }).await?;
    Ok(())
}

async fn run_restore(source: String, as_name: String, target_time: Option<String>, state_root: Option<PathBuf>) -> Result<()> {
    use crate::commands::restore::{run, RestoreArgs};
    run(RestoreArgs { source, as_name, target_time, override_state_root: state_root }).await?;
    Ok(())
}
