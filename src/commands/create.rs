use crate::config::global::GlobalConfig;
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::{
    BindMount, BuildImageSpec, CreateContainerSpec, DockerEngine, NamedVolume,
};
use crate::docker::image::dockerfile;
use crate::domain::instance::Instance;
use crate::domain::preset::Preset;
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::generate_pgbackrest_conf;
use crate::ports::{TcpProbe, allocate_port};
use crate::postgres::hba::generate_pg_hba;
use crate::postgres::init_sql::generate_init_sql;
use crate::state::instance::InstanceState;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CreateArgs {
    pub name: String,
    pub preset: Preset,
    pub pg_version: u8,
    pub app_user: String,
    pub app_password: String,
    pub pgbackrest_password: String,
    /// When None, uses GlobalConfig::default_path() and InstanceState::default_state_root().
    /// Tests set this to a TempDir.
    pub override_state_root: Option<PathBuf>,
    /// Local-only mode: skip pgbackrest setup entirely. The instance has no
    /// S3 backups, no archive_mode, no stanza-create. `pgforge snapshot`,
    /// `pgforge restore`, `pgforge clone` will refuse to operate on it.
    /// Intended for dev / test instances where S3 is unavailable.
    pub no_backup: bool,
    /// pgbackrest retention in days (passed through to Instance.retain_days
    /// and pgbackrest.conf). Default 30. 0 = keep all fulls forever.
    pub retain_days: u32,
    /// Auto-snapshot hour (0..=23 local time), or None for manual only.
    /// Default Some(3) = 03:00 local.
    pub snapshot_hour: Option<u8>,
}

pub struct ConfigLayout {
    pub root: PathBuf,
    pub postgresql_conf: PathBuf,
    pub pg_hba: PathBuf,
    pub pgbackrest_conf: PathBuf,
    pub init_sql: PathBuf,
}

impl ConfigLayout {
    pub fn for_instance(state_root: &Path, name: &str) -> Self {
        let root = state_root.join("instances").join(name).join("conf");
        Self {
            postgresql_conf: root.join("postgresql.conf"),
            pg_hba: root.join("pg_hba.conf"),
            pgbackrest_conf: root.join("pgbackrest.conf"),
            init_sql: root.join("init").join("01-pgforge-bootstrap.sql"),
            root,
        }
    }
}

/// Top-level entry called by main.rs / CLI.
pub async fn run(args: CreateArgs) -> Result<InstanceState> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let global_cfg = GlobalConfig::load()?;
    // S3 is required for backup-enabled instances (pgbackrest pushes WAL
    // there); for --no-backup we accept the global config without it.
    let s3 = if args.no_backup {
        global_cfg.s3.clone()
    } else {
        Some(global_cfg.s3.clone().ok_or_else(|| {
            PgForgeError::Anyhow(anyhow::anyhow!(
                "S3 settings missing in global config ({}). \
                 Add an [s3] section, or use --no-backup for a local-only instance.",
                crate::config::global::GlobalConfig::default_path().display()
            ))
        })?)
    };

    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root, global_cfg, s3).await
}

/// Inner function — engine injected so integration tests can swap it.
/// `s3` is `Some` for backup-enabled instances, `None` for `--no-backup`.
pub async fn run_with_engine<E: DockerEngine>(
    args: CreateArgs,
    docker: &E,
    state_root: PathBuf,
    global_cfg: GlobalConfig,
    s3: Option<crate::pgbackrest::conf::S3Settings>,
) -> Result<InstanceState> {
    // Guards — must run before any port allocation or file writes.
    Instance::validate_name(&args.name)?;
    if !args.no_backup && args.pgbackrest_password.is_empty() {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "pgbackrest_password is required for backup-enabled instances. \
             Set PGFORGE_PGBACKREST_PASSWORD or pass --pgbackrest-password, \
             or use --no-backup for a local-only instance."
        )));
    }

    if InstanceState::exists_under(&state_root, &args.name) {
        return Err(PgForgeError::InstanceExists(args.name.clone()));
    }

    let container_name = format!("pgforge_{}", args.name);
    if docker.container_exists(&container_name).await? {
        return Err(PgForgeError::InstanceExists(args.name.clone()));
    }

    let tuning = args.preset.tuning();

    // 1. Allocate a port avoiding ones we've handed out before.
    let taken: HashSet<u16> = InstanceState::list_under(&state_root)?
        .into_iter()
        .filter_map(|n| InstanceState::load_under(&state_root, &n).ok())
        .map(|s| s.instance.host_port)
        .collect();
    let host_port = allocate_port(
        global_cfg.port_range_start,
        global_cfg.port_range_end,
        &taken,
        &TcpProbe,
    )?;

    // 2. Render configs and write them to the per-instance config dir on host.
    // Conf root is 0700 — contains plaintext role password (init_sql) and S3
    // credentials (pgbackrest.conf), when backups are enabled.
    let layout = ConfigLayout::for_instance(&state_root, &args.name);
    crate::util::fs::create_secret_dir(&layout.root)?;
    let with_archive = !args.no_backup;
    std::fs::write(
        &layout.postgresql_conf,
        crate::postgres::conf::generate_postgresql_conf_with_archive(args.preset, with_archive),
    )
    .map_err(|e| PgForgeError::Io { path: layout.postgresql_conf.clone(), source: e })?;
    std::fs::write(&layout.pg_hba, generate_pg_hba(&args.name, &args.app_user))
        .map_err(|e| PgForgeError::Io { path: layout.pg_hba.clone(), source: e })?;
    if let Some(s3) = s3.as_ref() {
        // pgbackrest.conf carries S3 access_key + secret_key.
        crate::util::fs::write_secret(
            &layout.pgbackrest_conf,
            generate_pgbackrest_conf(&args.name, s3, args.retain_days),
        )?;
        let init_dir = layout.init_sql.parent().unwrap().to_path_buf();
        crate::util::fs::create_secret_dir(&init_dir)?;
        // init_sql carries CREATE ROLE … PASSWORD '…' in plaintext.
        crate::util::fs::write_secret(
            &layout.init_sql,
            generate_init_sql(&args.pgbackrest_password),
        )?;
    }
    // For --no-backup we skip pgbackrest.conf and init_sql entirely. The
    // pg_hba file still references pgbackrest/pgreplica roles in its `local`
    // and `host replication` lines, but they simply won't match anything
    // since the roles never get created — that's a no-op, not an error.

    // 3. Make sure the per-version image exists.
    docker
        .build_image(&BuildImageSpec {
            tag: format!("pgforge/postgres:{}", args.pg_version),
            dockerfile: dockerfile(args.pg_version),
        })
        .await?;

    // 4. Make sure the shared docker network exists.
    docker.ensure_network("pgforge_net").await?;

    // 5. Create the container.
    let mut env = HashMap::new();
    env.insert("POSTGRES_USER".into(), args.app_user.clone());
    env.insert("POSTGRES_PASSWORD".into(), args.app_password.clone());
    env.insert("POSTGRES_DB".into(), args.name.clone());
    env.insert("PGDATA".into(), "/var/lib/postgresql/data/pgdata".into());

    let mut binds = vec![
        BindMount {
            host_path: layout.postgresql_conf.clone(),
            container_path: "/etc/postgresql/postgresql.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: layout.pg_hba.clone(),
            container_path: "/etc/postgresql/pg_hba.conf".into(),
            read_only: true,
        },
    ];
    if !args.no_backup {
        binds.push(BindMount {
            host_path: layout.pgbackrest_conf.clone(),
            container_path: "/etc/pgbackrest/pgbackrest.conf".into(),
            read_only: true,
        });
        binds.push(BindMount {
            host_path: layout.init_sql.clone(),
            container_path: "/docker-entrypoint-initdb.d/01-pgforge-bootstrap.sql".into(),
            read_only: true,
        });
    }
    let volumes = vec![NamedVolume {
        volume_name: format!("pgforge_data_{}", args.name),
        container_path: "/var/lib/postgresql/data".into(),
    }];

    let spec = CreateContainerSpec {
        container_name: format!("pgforge_{}", args.name),
        image: format!("pgforge/postgres:{}", args.pg_version),
        env,
        binds,
        volumes,
        host_port,
        container_port: 5432,
        memory_mb: tuning.ram_mb,
        network: "pgforge_net".into(),
        shm_size_mb: 256,
        // Keep image's docker-entrypoint.sh (it handles initdb + init-SQL
        // hooks). Override CMD only, to point postgres at the bind-mounted
        // configs — without this, our generated postgresql.conf and
        // pg_hba.conf are inert (docker-entrypoint reads only $PGDATA
        // contents).
        entrypoint_override: None,
        cmd_override: Some(vec![
            "postgres".into(),
            "-c".into(),
            "config_file=/etc/postgresql/postgresql.conf".into(),
            "-c".into(),
            "hba_file=/etc/postgresql/pg_hba.conf".into(),
        ]),
        restart_policy: crate::docker::engine::RestartPolicy::UnlessStopped,
    };
    let container_name = spec.container_name.clone();
    let volume_name = spec.volumes[0].volume_name.clone();
    let id = docker.create_container(&spec).await?;

    // From here on, any failure should clean up the half-created container + volume + conf dir.
    let conf_dir = layout.root.clone();
    let state = match bootstrap_create(docker, &id, &args, host_port).await {
        Ok(state) => state,
        Err(e) => {
            crate::docker::cleanup::cleanup_partial(
                docker,
                &container_name,
                &volume_name,
                &conf_dir,
            )
            .await;
            return Err(e);
        }
    };

    // state.save_under is OUTSIDE the cleanup wrap — a fully-bootstrapped
    // container shouldn't be destroyed because of a local filesystem error.
    if let Err(e) = state.save_under_locked(&state_root) {
        tracing::error!(
            target: "pgforge::create",
            "instance {} bootstrapped successfully but state.toml save failed: {e}. \
             Container {container_name} is running on port {host_port}; resave state \
             manually or rerun once the filesystem is healthy.",
            args.name
        );
        return Err(e);
    }
    Ok(state)
}

/// All steps after `create_container` that must rollback on failure: start,
/// wait for pg ready, create pgbackrest stanza. Returns the in-memory state
/// — saving it to disk is the caller's responsibility (kept outside the
/// cleanup wrap so a healthy container survives a state-save error).
async fn bootstrap_create<E: DockerEngine>(
    docker: &E,
    id: &str,
    args: &CreateArgs,
    host_port: u16,
) -> Result<InstanceState> {
    // 6. Start it.
    docker.start_container(id).await?;

    // Wait for the container to reach Running state in Docker's eyes, then for
    // Postgres inside it to accept local connections, then create the
    // pgbackrest stanza so archive_command can begin pushing WAL.
    docker
        .wait_for_container_running(id, std::time::Duration::from_secs(30))
        .await?;
    crate::docker::wait::wait_for_pg_ready(docker, id, 30).await?;
    // pgbackrest stanza-create is the gate that turns archive_command from
    // a doomed retry-storm into a working WAL pipeline. Skip it when there
    // is no pgbackrest at all (--no-backup).
    //
    // Race: once postgres is `pg_isready`, the first checkpoint switches a
    // WAL segment and fires archive_command (pgbackrest archive-push),
    // which grabs /tmp/pgbackrest/main-archive-1.lock. If our stanza-create
    // races into the same lock we get exit 50. We retry a few times — the
    // archive-push holds the lock for <1 s so a short sleep gets us in.
    if !args.no_backup {
        let mut last_err: Option<String> = None;
        let mut last_exit: i64 = 0;
        let mut last_out = (String::new(), String::new());
        for attempt in 0..5u8 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
            let stanza = docker
                .exec(
                    id,
                    &[
                        "su", "-", "postgres", "-c",
                        "pgbackrest --stanza=main stanza-create",
                    ],
                )
                .await?;
            if stanza.exit_code == 0 {
                last_err = None;
                break;
            }
            last_exit = stanza.exit_code;
            last_out = (stanza.stdout.clone(), stanza.stderr.clone());
            // Only retry the lock-contention path; other errors (bad
            // credentials, bucket missing) won't get better on retry.
            let lock_busy = stanza.stdout.contains("unable to acquire lock")
                || stanza.stderr.contains("unable to acquire lock");
            if !lock_busy {
                last_err = Some("non-lock error".into());
                break;
            }
            last_err = Some("lock busy".into());
            tracing::warn!(
                target: "pgforge::create",
                "stanza-create attempt {} hit archive-push lock; retrying",
                attempt + 1
            );
        }
        if last_err.is_some() {
            return Err(PgForgeError::Docker(format!(
                "pgbackrest stanza-create failed (exit {}): stdout={:?} stderr={:?}",
                last_exit, last_out.0, last_out.1
            )));
        }
    }

    Ok(InstanceState {
        instance: Instance {
            name: args.name.clone(),
            db_name: args.name.clone(),
            app_user: args.app_user.clone(),
            app_password: args.app_password.clone(),
            pgbackrest_password: args.pgbackrest_password.clone(),
            preset: args.preset,
            pg_version: args.pg_version,
            host_port,
            backup_enabled: !args.no_backup,
            volume_name_override: None,
            retain_days: args.retain_days,
            snapshot_hour: args.snapshot_hour,
            last_snapshot_at: None,
            last_snapshot_attempt_at: None,
            full_backup_day: 0,
        },
        created_at: crate::time::now_iso(),
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::global::GlobalConfig;
    use crate::docker::engine::{BuildImageSpec, CreateContainerSpec, DockerEngine};
    use crate::pgbackrest::conf::S3Settings;
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::Mutex;

    struct RecordingEngine {
        calls: Mutex<Vec<&'static str>>,
    }

    #[async_trait]
    impl DockerEngine for RecordingEngine {
        async fn build_image(&self, _: &BuildImageSpec) -> crate::error::Result<()> {
            self.calls.lock().unwrap().push("build_image");
            Ok(())
        }
        async fn ensure_network(&self, _: &str) -> crate::error::Result<()> {
            self.calls.lock().unwrap().push("ensure_network");
            Ok(())
        }
        async fn create_container(&self, _: &CreateContainerSpec) -> crate::error::Result<String> {
            self.calls.lock().unwrap().push("create_container");
            Ok("dummy_id".into())
        }
        async fn start_container(&self, _: &str) -> crate::error::Result<()> {
            self.calls.lock().unwrap().push("start_container");
            Ok(())
        }
        async fn container_exists(&self, _: &str) -> crate::error::Result<bool> {
            self.calls.lock().unwrap().push("container_exists");
            Ok(false)
        }
        async fn container_running(&self, _: &str) -> crate::error::Result<bool> {
            self.calls.lock().unwrap().push("container_running");
            Ok(true)
        }
        async fn exec(&self, _: &str, _: &[&str]) -> crate::error::Result<crate::docker::engine::ExecOutput> {
            self.calls.lock().unwrap().push("exec");
            Ok(crate::docker::engine::ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn exec_as(&self, container: &str, _user: &str, cmd: &[&str]) -> crate::error::Result<crate::docker::engine::ExecOutput> {
            // Mock: delegate to exec, ignoring the user argument.
            self.exec(container, cmd).await
        }
        async fn exec_with_stdin(&self, container: &str, cmd: &[&str], _stdin_data: &str) -> crate::error::Result<crate::docker::engine::ExecOutput> {
            // Mock: delegate to exec, ignoring stdin_data.
            self.exec(container, cmd).await
        }
        async fn exec_to_file(
            &self,
            _: &str,
            _: &[&str],
            _: &std::path::Path,
        ) -> crate::error::Result<crate::docker::engine::ExecToFileOutput> {
            self.calls.lock().unwrap().push("exec_to_file");
            Ok(crate::docker::engine::ExecToFileOutput {
                exit_code: 0,
                stderr: String::new(),
            })
        }
        async fn stop_container(&self, _: &str) -> crate::error::Result<()> {
            self.calls.lock().unwrap().push("stop_container");
            Ok(())
        }
        async fn wait_for_container_running(
            &self,
            _: &str,
            _: std::time::Duration,
        ) -> crate::error::Result<()> {
            self.calls.lock().unwrap().push("wait_for_container_running");
            Ok(())
        }
        async fn wait_for_container_exit(
            &self,
            _: &str,
            _: std::time::Duration,
        ) -> crate::error::Result<i64> {
            self.calls.lock().unwrap().push("wait_for_container_exit");
            Ok(0)
        }
        async fn remove_container(&self, _: &str, _: bool) -> crate::error::Result<()> {
            self.calls.lock().unwrap().push("remove_container");
            Ok(())
        }
        async fn remove_volume(&self, _: &str) -> crate::error::Result<()> {
            self.calls.lock().unwrap().push("remove_volume");
            Ok(())
        }
        async fn inspect_container(&self, _name: &str) -> crate::error::Result<crate::docker::engine::ContainerInspect> {
            Ok(crate::docker::engine::ContainerInspect::default())
        }
        async fn logs(&self, _container: &str) -> crate::error::Result<String> {
            Ok(String::new())
        }
    }

    #[tokio::test]
    async fn invalid_name_short_circuits_before_any_docker_call() {
        let tmp = tempfile::TempDir::new().unwrap();
        let engine = RecordingEngine { calls: Mutex::new(Vec::new()) };
        let s3 = S3Settings {
            bucket: "b".into(),
            region: "r".into(),
            endpoint: "e".into(),
            access_key: "a".into(),
            secret_key: "s".into(),
        };
        let cfg = GlobalConfig { s3: Some(s3.clone()), ..Default::default() };
        let res = run_with_engine(
            CreateArgs {
                name: "INVALID-UPPERCASE".into(),
                preset: crate::domain::preset::Preset::Tiny,
                pg_version: 18,
                app_user: "leads".into(),
                app_password: "pw".into(),
                pgbackrest_password: "rpw".into(),
                override_state_root: Some(tmp.path().to_path_buf()),
                no_backup: false,
                snapshot_hour: None,
                retain_days: 30,
            },
            &engine,
            tmp.path().to_path_buf(),
            cfg,
            Some(s3),
        )
        .await;
        assert!(res.is_err());
        assert!(
            engine.calls.lock().unwrap().is_empty(),
            "no docker call should happen on invalid name, got {:?}",
            engine.calls.lock().unwrap()
        );
    }

    #[test]
    fn config_layout_is_per_instance() {
        let layout = ConfigLayout::for_instance(Path::new("/state"), "billing");
        assert_eq!(
            layout.postgresql_conf,
            PathBuf::from("/state/instances/billing/conf/postgresql.conf"),
        );
        assert_eq!(
            layout.pg_hba,
            PathBuf::from("/state/instances/billing/conf/pg_hba.conf"),
        );
        assert_eq!(
            layout.pgbackrest_conf,
            PathBuf::from("/state/instances/billing/conf/pgbackrest.conf"),
        );
        assert_eq!(
            layout.init_sql,
            PathBuf::from("/state/instances/billing/conf/init/01-pgforge-bootstrap.sql"),
        );
    }

}
