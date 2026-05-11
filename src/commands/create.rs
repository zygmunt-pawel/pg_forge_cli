use crate::config::global::GlobalConfig;
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::{
    BindMount, BuildImageSpec, CreateContainerSpec, DockerEngine, NamedVolume,
};
use crate::docker::image::dockerfile;
use crate::domain::instance::Instance;
use crate::domain::platform::current_platform;
use crate::domain::preset::Preset;
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::generate_pgbackrest_conf;
use crate::ports::{TcpProbe, allocate_port};
use crate::postgres::conf::generate_postgresql_conf;
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
    let s3 = global_cfg
        .s3
        .clone()
        .ok_or_else(|| PgForgeError::Anyhow(anyhow::anyhow!(
            "S3 settings missing in global config (~/.config/pgforge/config.toml). Add an [s3] section."
        )))?;

    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root, global_cfg, s3.clone()).await
}

/// Inner function — engine injected so integration tests can swap it.
pub async fn run_with_engine<E: DockerEngine>(
    args: CreateArgs,
    docker: &E,
    state_root: PathBuf,
    global_cfg: GlobalConfig,
    s3: crate::pgbackrest::conf::S3Settings,
) -> Result<InstanceState> {
    // Guards — must run before any port allocation or file writes.
    Instance::validate_name(&args.name)?;

    if InstanceState::exists_under(&state_root, &args.name) {
        return Err(PgForgeError::InstanceExists(args.name.clone()));
    }

    let container_name = format!("pgforge_{}", args.name);
    if docker.container_exists(&container_name).await? {
        return Err(PgForgeError::InstanceExists(args.name.clone()));
    }

    let plat = current_platform();
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
    let layout = ConfigLayout::for_instance(&state_root, &args.name);
    std::fs::create_dir_all(&layout.root).map_err(|e| PgForgeError::Io {
        path: layout.root.clone(),
        source: e,
    })?;
    std::fs::write(&layout.postgresql_conf, generate_postgresql_conf(args.preset, plat))
        .map_err(|e| PgForgeError::Io { path: layout.postgresql_conf.clone(), source: e })?;
    std::fs::write(&layout.pg_hba, generate_pg_hba(&args.name, &args.app_user))
        .map_err(|e| PgForgeError::Io { path: layout.pg_hba.clone(), source: e })?;
    std::fs::write(&layout.pgbackrest_conf, generate_pgbackrest_conf(&args.name, &s3))
        .map_err(|e| PgForgeError::Io { path: layout.pgbackrest_conf.clone(), source: e })?;
    let init_dir = layout.init_sql.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&init_dir).map_err(|e| PgForgeError::Io { path: init_dir.clone(), source: e })?;
    std::fs::write(&layout.init_sql, generate_init_sql(&args.pgbackrest_password))
        .map_err(|e| PgForgeError::Io { path: layout.init_sql.clone(), source: e })?;

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

    let binds = vec![
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
        BindMount {
            host_path: layout.pgbackrest_conf.clone(),
            container_path: "/etc/pgbackrest/pgbackrest.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: layout.init_sql.clone(),
            container_path: "/docker-entrypoint-initdb.d/01-pgforge-bootstrap.sql".into(),
            read_only: true,
        },
    ];
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
        command_override: None,
    };
    let id = docker.create_container(&spec).await?;

    // 6. Start it.
    docker.start_container(&id).await?;

    // Wait for the container to reach Running state in Docker's eyes, then for
    // Postgres inside it to accept local connections, then create the
    // pgbackrest stanza so archive_command can begin pushing WAL.
    docker
        .wait_for_container_running(&id, std::time::Duration::from_secs(30))
        .await?;
    wait_for_pg_ready(docker, &id).await?;
    let stanza = docker
        .exec(
            &id,
            &[
                "su", "-", "postgres", "-c",
                "pgbackrest --stanza=main stanza-create",
            ],
        )
        .await?;
    if stanza.exit_code != 0 {
        return Err(PgForgeError::Docker(format!(
            "pgbackrest stanza-create failed (exit {}): stdout={:?} stderr={:?}",
            stanza.exit_code, stanza.stdout, stanza.stderr
        )));
    }

    // 7. Persist state.
    let state = InstanceState {
        instance: Instance {
            name: args.name.clone(),
            db_name: args.name.clone(),
            app_user: args.app_user,
            app_password: args.app_password,
            pgbackrest_password: args.pgbackrest_password,
            preset: args.preset,
            pg_version: args.pg_version,
            host_port,
        },
        created_at: crate::time::now_iso(),
    };
    state.save_under(&state_root)?;

    Ok(state)
}


async fn wait_for_pg_ready<E: DockerEngine>(docker: &E, id: &str) -> Result<()> {
    for _ in 0..30 {
        let out = docker
            .exec(id, &["pg_isready", "-h", "/var/run/postgresql"])
            .await?;
        if out.exit_code == 0 {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err(PgForgeError::Docker(format!(
        "container {id}: postgres did not accept connections within 30s"
    )))
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
        async fn exec(&self, _: &str, _: &[&str]) -> crate::error::Result<crate::docker::engine::ExecOutput> {
            self.calls.lock().unwrap().push("exec");
            Ok(crate::docker::engine::ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
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
            },
            &engine,
            tmp.path().to_path_buf(),
            cfg,
            s3,
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
