//! `pgforge resize --name X --preset <new>` — change an instance's
//! tuning preset (RAM limit, max_connections, shared_buffers, etc.).
//!
//! Implementation: mutate `preset` in state.toml, save, then call
//! `commands::rotate::run`. Rotate regenerates postgresql.conf from
//! the current state + recreates the container with the new memory
//! limit, all while keeping the data volume. ~10s downtime, same as
//! a plain rotate.
//!
//! Rollback: if rotate fails (e.g. docker unreachable), we restore
//! the old preset in state.toml so the next `pgforge ls` / rotate
//! still reflects reality. Not transactional — if the system goes
//! down mid-save the state.toml could be inconsistent with the
//! running container, but Plan 1's save_under uses atomic write.

use crate::domain::instance::Instance;
use crate::domain::preset::Preset;
use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ResizeArgs {
    pub name: String,
    pub new_preset: Preset,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: ResizeArgs) -> Result<(Preset, Preset)> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);

    Instance::validate_name(&args.name)?;
    let old_preset = InstanceState::load_under(&state_root, &args.name)?
        .instance
        .preset;
    if old_preset == args.new_preset {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "{:?} is already on preset {:?} — nothing to resize",
            args.name,
            old_preset
        )));
    }

    InstanceState::update_under(&state_root, &args.name, |s| {
        s.instance.preset = args.new_preset;
        Ok(())
    })?;

    let rotate_args = crate::commands::rotate::RotateArgs {
        name: args.name.clone(),
        override_state_root: args.override_state_root.clone(),
    };
    match crate::commands::rotate::run(rotate_args).await {
        Ok(()) => Ok((old_preset, args.new_preset)),
        Err(e) => {
            // Best-effort rollback so state.toml doesn't claim a preset
            // the container never actually applied.
            let _ = InstanceState::update_under(&state_root, &args.name, |s| {
                s.instance.preset = old_preset;
                Ok(())
            });
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_state(state_root: &std::path::Path, name: &str, preset: Preset) {
        let s = InstanceState {
            instance: Instance {
                name: name.into(),
                db_name: name.into(),
                app_user: "app".into(),
                app_password: "pw".into(),
                pgbackrest_password: String::new(),
                preset,
                pg_version: 18,
                host_port: 5433,
                backup_enabled: false,
                volume_name_override: None,
                retain_days: 30,
                snapshot_hour: Some(3),
                last_snapshot_at: None,
                last_snapshot_attempt_at: None,
                full_backup_day: 0,
            },
            created_at: "2026-05-12T10:00:00Z".into(),
        };
        s.save_under(state_root).unwrap();
    }

    #[tokio::test]
    async fn same_preset_short_circuits_with_error() {
        let tmp = TempDir::new().unwrap();
        write_state(tmp.path(), "alpha", Preset::Small);
        let err = run(ResizeArgs {
            name: "alpha".into(),
            new_preset: Preset::Small,
            override_state_root: Some(tmp.path().to_path_buf()),
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("already on preset"));
        // State.toml unchanged.
        let st = InstanceState::load_under(tmp.path(), "alpha").unwrap();
        assert_eq!(st.instance.preset, Preset::Small);
    }

    #[tokio::test]
    async fn missing_instance_fails_cleanly() {
        let tmp = TempDir::new().unwrap();
        let err = run(ResizeArgs {
            name: "ghost".into(),
            new_preset: Preset::Medium,
            override_state_root: Some(tmp.path().to_path_buf()),
        })
        .await
        .unwrap_err();
        assert!(matches!(err, PgForgeError::InstanceNotFound(_)));
    }

    // We don't run a full rotate-success path in unit tests — that
    // would require a live docker engine. The CLI E2E suite exercises
    // it through `pgforge rotate` already; resize delegates verbatim.
}
