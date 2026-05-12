//! `pgforge destroy --name <X>` — permanently delete an instance.
//!
//! Workflow:
//!   1. Load state.toml (errors out if instance doesn't exist).
//!   2. If `--delete-backups` AND the instance is backup-enabled AND the
//!      container is running: exec `pgbackrest --stanza=main --force
//!      stanza-delete` inside the container. pgbackrest natively removes
//!      the entire stanza prefix from S3 (full backups + diff + WAL
//!      archives + info files — everything PITR depends on). Without
//!      this flag, S3 data is preserved.
//!   3. Stop the container if running, then remove it.
//!   4. Remove the data volume (this is the irreversible local data loss).
//!   5. Remove the per-instance state dir under
//!      `<state_root>/instances/<name>/` (state.toml, conf, secrets).
//!
//! Caller is responsible for confirmation prompt — this function does
//! the destructive work without asking.

use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::domain::instance::Instance;
use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct DestroyArgs {
    pub name: String,
    /// If true and the instance is backup-enabled, run `pgbackrest
    /// stanza-delete` before removing the container — this wipes the
    /// stanza's full backups and WAL archives from S3 (no PITR
    /// recoverable afterwards).
    pub delete_backups: bool,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: DestroyArgs) -> Result<()> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: DestroyArgs,
    docker: &E,
    state_root: PathBuf,
) -> Result<()> {
    Instance::validate_name(&args.name)?;
    let state = InstanceState::load_under(&state_root, &args.name)?;
    let instance = &state.instance;
    let container_name = format!("pgforge_{}", instance.name);
    let volume_name = instance.volume_name();

    // 1. Optionally wipe S3 backups. Must happen BEFORE container shutdown:
    // pgbackrest needs to be exec'd inside the running container (that's
    // where /etc/pgbackrest/pgbackrest.conf and the S3 credentials live).
    if args.delete_backups {
        if !instance.backup_enabled {
            tracing::warn!(
                target: "pgforge::destroy",
                "instance {:?} is --no-backup; --delete-backups has nothing to clean", instance.name
            );
        } else if !docker.container_running(&container_name).await? {
            return Err(PgForgeError::Anyhow(anyhow::anyhow!(
                "cannot delete S3 backups for {:?}: container is not running. \
                 Start it first (pgforge rotate) or destroy without --delete-backups \
                 and clean the S3 prefix manually.",
                instance.name
            )));
        } else {
            // pgbackrest stanza-delete refuses to run unless a stop file
            // exists for the stanza (safety: makes sure no concurrent
            // archive-push / backup races the deletion). `pgbackrest stop`
            // creates that file and waits for in-flight commands; then
            // stanza-delete --force wipes the S3 prefix.
            let out = docker
                .exec(
                    &container_name,
                    &[
                        "su",
                        "-",
                        "postgres",
                        "-c",
                        "pgbackrest --stanza=main stop && \
                         pgbackrest --stanza=main --force stanza-delete",
                    ],
                )
                .await?;
            if out.exit_code != 0 {
                return Err(PgForgeError::Anyhow(anyhow::anyhow!(
                    "pgbackrest stanza-delete failed (exit {}): stdout={} stderr={}",
                    out.exit_code,
                    out.stdout,
                    out.stderr
                )));
            }
            tracing::info!(
                target: "pgforge::destroy",
                "deleted S3 stanza for {:?}", instance.name
            );
        }
    }

    // 2. Stop + remove container.
    if docker.container_running(&container_name).await? {
        docker.stop_container(&container_name).await?;
    }
    if docker.container_exists(&container_name).await? {
        docker.remove_container(&container_name, true).await?;
    }

    // 3. Remove data volume.
    docker.remove_volume(&volume_name).await?;

    // 4. Remove state dir (state.toml + conf/ + secrets).
    let inst_dir = state_root.join("instances").join(&args.name);
    if inst_dir.exists() {
        std::fs::remove_dir_all(&inst_dir).map_err(|e| PgForgeError::Io {
            path: inst_dir,
            source: e,
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::engine::{
        BindMount, BuildImageSpec, CreateContainerSpec, ExecOutput, NamedVolume,
    };
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Minimal mock DockerEngine that records the operations it received
    /// and lets each predicate be steered from the test.
    #[derive(Default)]
    struct MockDocker {
        ops: Mutex<Vec<String>>,
        running: bool,
        exists: bool,
        exec_exit: i64,
    }

    #[async_trait]
    impl DockerEngine for MockDocker {
        async fn build_image(&self, _: &BuildImageSpec) -> Result<()> { Ok(()) }
        async fn ensure_network(&self, _: &str) -> Result<()> { Ok(()) }
        async fn create_container(&self, _: &CreateContainerSpec) -> Result<String> { Ok("id".into()) }
        async fn start_container(&self, _: &str) -> Result<()> { Ok(()) }
        async fn container_exists(&self, name: &str) -> Result<bool> {
            self.ops.lock().unwrap().push(format!("exists({name})"));
            Ok(self.exists)
        }
        async fn container_running(&self, name: &str) -> Result<bool> {
            self.ops.lock().unwrap().push(format!("running({name})"));
            Ok(self.running)
        }
        async fn exec(&self, name: &str, cmd: &[&str]) -> Result<ExecOutput> {
            self.ops.lock().unwrap().push(format!("exec({name}, {})", cmd.join(" ")));
            Ok(ExecOutput { stdout: String::new(), stderr: String::new(), exit_code: self.exec_exit })
        }
        async fn stop_container(&self, name: &str) -> Result<()> {
            self.ops.lock().unwrap().push(format!("stop({name})"));
            Ok(())
        }
        async fn wait_for_container_running(&self, _: &str, _: Duration) -> Result<()> { Ok(()) }
        async fn wait_for_container_exit(&self, _: &str, _: Duration) -> Result<i64> { Ok(0) }
        async fn remove_container(&self, name: &str, _force: bool) -> Result<()> {
            self.ops.lock().unwrap().push(format!("rm_container({name})"));
            Ok(())
        }
        async fn remove_volume(&self, name: &str) -> Result<()> {
            self.ops.lock().unwrap().push(format!("rm_volume({name})"));
            Ok(())
        }
        async fn inspect_container(&self, _name: &str) -> Result<crate::docker::engine::ContainerInspect> {
            Ok(crate::docker::engine::ContainerInspect::default())
        }
    }

    fn write_state(state_root: &std::path::Path, name: &str, backup_enabled: bool) {
        use crate::domain::preset::Preset;
        let s = InstanceState {
            instance: Instance {
                name: name.into(),
                db_name: name.into(),
                app_user: "leads".into(),
                app_password: "pw".into(),
                pgbackrest_password: String::new(),
                preset: Preset::Tiny,
                pg_version: 18,
                host_port: 5433,
                backup_enabled,
                volume_name_override: None,
            },
            created_at: "2026-05-12T10:00:00Z".into(),
        };
        s.save_under(state_root).unwrap();
    }

    #[tokio::test]
    async fn destroys_container_volume_and_state() {
        let tmp = TempDir::new().unwrap();
        write_state(tmp.path(), "alpha", false);
        let docker = MockDocker { exists: true, running: true, ..Default::default() };

        run_with_engine(
            DestroyArgs {
                name: "alpha".into(),
                delete_backups: false,
                override_state_root: Some(tmp.path().to_path_buf()),
            },
            &docker,
            tmp.path().to_path_buf(),
        )
        .await
        .unwrap();

        let ops = docker.ops.lock().unwrap();
        assert!(ops.iter().any(|o| o == "stop(pgforge_alpha)"));
        assert!(ops.iter().any(|o| o == "rm_container(pgforge_alpha)"));
        assert!(ops.iter().any(|o| o == "rm_volume(pgforge_data_alpha)"));
        // No exec call when delete_backups=false.
        assert!(!ops.iter().any(|o| o.starts_with("exec(")));
        // State dir is gone.
        assert!(!tmp.path().join("instances/alpha").exists());
    }

    #[tokio::test]
    async fn delete_backups_runs_pgbackrest_stanza_delete_before_destroy() {
        let tmp = TempDir::new().unwrap();
        write_state(tmp.path(), "beta", true /* backup_enabled */);
        let docker = MockDocker {
            exists: true,
            running: true,
            exec_exit: 0,
            ..Default::default()
        };

        run_with_engine(
            DestroyArgs {
                name: "beta".into(),
                delete_backups: true,
                override_state_root: Some(tmp.path().to_path_buf()),
            },
            &docker,
            tmp.path().to_path_buf(),
        )
        .await
        .unwrap();

        let ops = docker.ops.lock().unwrap();
        let exec_pos = ops.iter().position(|o| o.contains("stanza-delete")).expect("stanza-delete must be called");
        let stop_pos = ops.iter().position(|o| o == "stop(pgforge_beta)").expect("stop must be called");
        assert!(exec_pos < stop_pos, "stanza-delete must run BEFORE container stop");
    }

    #[tokio::test]
    async fn delete_backups_fails_if_container_not_running() {
        let tmp = TempDir::new().unwrap();
        write_state(tmp.path(), "gamma", true);
        let docker = MockDocker { exists: true, running: false, ..Default::default() };

        let err = run_with_engine(
            DestroyArgs {
                name: "gamma".into(),
                delete_backups: true,
                override_state_root: Some(tmp.path().to_path_buf()),
            },
            &docker,
            tmp.path().to_path_buf(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("container is not running"));
        // State must NOT be removed on early-out.
        assert!(tmp.path().join("instances/gamma").exists());
    }

    #[tokio::test]
    async fn delete_backups_noop_on_no_backup_instance() {
        let tmp = TempDir::new().unwrap();
        write_state(tmp.path(), "delta", false /* not backup_enabled */);
        let docker = MockDocker { exists: true, running: true, ..Default::default() };

        run_with_engine(
            DestroyArgs {
                name: "delta".into(),
                delete_backups: true, // requested, but no S3 to clean
                override_state_root: Some(tmp.path().to_path_buf()),
            },
            &docker,
            tmp.path().to_path_buf(),
        )
        .await
        .unwrap();

        let ops = docker.ops.lock().unwrap();
        // No stanza-delete because backup_enabled=false.
        assert!(!ops.iter().any(|o| o.contains("stanza-delete")));
        // But destroy still proceeds.
        assert!(ops.iter().any(|o| o == "rm_volume(pgforge_data_delta)"));
    }

    #[tokio::test]
    async fn missing_instance_errors_out() {
        let tmp = TempDir::new().unwrap();
        let docker = MockDocker::default();
        let err = run_with_engine(
            DestroyArgs {
                name: "ghost".into(),
                delete_backups: false,
                override_state_root: Some(tmp.path().to_path_buf()),
            },
            &docker,
            tmp.path().to_path_buf(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PgForgeError::InstanceNotFound(_)));
    }
}
