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
            OpKind::Resize => {
                // Encoding: "name@<preset_name>"
                use std::str::FromStr;
                let (name, suffix) = match encoded.split_once('@') {
                    Some((n, s)) => (n.to_string(), s.to_string()),
                    None         => (encoded.clone(), String::new()),
                };
                match crate::domain::preset::Preset::from_str(&suffix) {
                    Ok(p) => (name.clone(), Box::pin(run_resize(name, p, state_root))),
                    Err(e) => {
                        let _ = tx.send(Event::OpFinished {
                            instance: name,
                            kind: OpKind::Resize,
                            result: Err(format!("invalid preset encoding {:?}: {e}", suffix)),
                        });
                        return;
                    }
                }
            }
            OpKind::Destroy => {
                // Encoding: "name" = keep S3 backups; "name@delete" = wipe.
                let (name, suffix) = match encoded.split_once('@') {
                    Some((n, s)) => (n.to_string(), s.to_string()),
                    None         => (encoded.clone(), String::new()),
                };
                let delete_backups = suffix == "delete";
                (name.clone(), Box::pin(run_destroy(name, delete_backups, state_root)))
            }
            OpKind::Clipboard | OpKind::Create => {
                // Clipboard is sync and never spawned via this entry
                // point; Create is dispatched via `spawn_create` (its
                // arg list doesn't fit the (encoded, kind) shape).
                // Both branches are unreachable in normal flow but
                // must be exhaustive.
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

async fn run_destroy(name: String, delete_backups: bool, state_root: Option<PathBuf>) -> Result<()> {
    use crate::commands::destroy::{run, DestroyArgs};
    run(DestroyArgs { name, delete_backups, override_state_root: state_root }).await?;
    Ok(())
}

async fn run_resize(name: String, new_preset: crate::domain::preset::Preset, state_root: Option<PathBuf>) -> Result<()> {
    use crate::commands::resize::{run, ResizeArgs};
    run(ResizeArgs { name, new_preset, override_state_root: state_root }).await?;
    Ok(())
}

/// Dispatch a CreateRequest from `AppState::pending_creates`. Wraps
/// commands::create with the same OpStarted/OpFinished envelope as
/// other long-ops so the lock/spinner/flash machinery works.
pub fn spawn_create(
    req: crate::tui::events::CreateRequest,
    tx: tokio::sync::mpsc::UnboundedSender<crate::tui::events::Event>,
    state_root: Option<PathBuf>,
) {
    let name = req.name.clone();
    tokio::spawn(async move {
        let _ = tx.send(crate::tui::events::Event::OpStarted {
            instance: name.clone(),
            kind: crate::tui::events::OpKind::Create,
        });
        let result = run_create(req, state_root).await.map_err(|e| e.to_string());
        let _ = tx.send(crate::tui::events::Event::OpFinished {
            instance: name,
            kind: crate::tui::events::OpKind::Create,
            result,
        });
    });
}

async fn run_create(req: crate::tui::events::CreateRequest, state_root: Option<PathBuf>) -> Result<()> {
    use crate::commands::create::{run as run_cmd, CreateArgs};
    run_cmd(CreateArgs {
        name: req.name,
        preset: req.preset,
        pg_version: req.pg_version,
        app_user: req.app_user,
        app_password: req.app_password,
        pgbackrest_password: req.pgbackrest_password,
        override_state_root: state_root,
        no_backup: req.no_backup,
        retain_days: req.retain_days,
    })
    .await?;
    Ok(())
}
