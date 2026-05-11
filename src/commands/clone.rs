use crate::config::global::GlobalConfig;
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::cleanup::cleanup_partial;
use crate::docker::clone_entrypoint::generate_clone_entrypoint;
use crate::docker::engine::{
    BindMount, BuildImageSpec, CreateContainerSpec, DockerEngine, NamedVolume,
};
use crate::docker::image::dockerfile;
use crate::docker::wait::wait_for_pg_ready;
use crate::domain::instance::Instance;
use crate::domain::platform::current_platform;
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::generate_pgbackrest_conf;
use crate::pgbackrest::pgpass::generate_pgpass;
use crate::ports::{TcpProbe, allocate_port};
use crate::postgres::conf::generate_postgresql_conf;
use crate::postgres::hba::generate_pg_hba;
use crate::state::instance::InstanceState;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CloneArgs {
    pub source: String,
    pub as_name: String,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: CloneArgs) -> Result<InstanceState> {
    Instance::validate_name(&args.as_name)?;
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let global = GlobalConfig::load()?;
    let s3 = global
        .s3
        .as_ref()
        .ok_or_else(|| {
            PgForgeError::Anyhow(anyhow::anyhow!("S3 settings missing in global config"))
        })?
        .clone();
    let source = InstanceState::load_under(&state_root, &args.source)?;
    if InstanceState::exists_under(&state_root, &args.as_name) {
        return Err(PgForgeError::InstanceExists(args.as_name.clone()));
    }
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root, global, s3, source).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: CloneArgs,
    docker: &E,
    state_root: PathBuf,
    global: GlobalConfig,
    s3: crate::pgbackrest::conf::S3Settings,
    source: InstanceState,
) -> Result<InstanceState> {
    // Source must be running so pg_basebackup can connect.
    let source_container = format!("pgforge_{}", args.source);
    if !docker.container_running(&source_container).await? {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "source instance {:?} is not running. Start it with `docker start {source_container}` and retry.",
            args.source
        )));
    }

    let plat = current_platform();
    let tuning = source.instance.preset.tuning();

    // Port allocation, skipping ports already taken.
    let taken: HashSet<u16> = InstanceState::list_under(&state_root)?
        .into_iter()
        .filter_map(|n| InstanceState::load_under(&state_root, &n).ok())
        .map(|s| s.instance.host_port)
        .collect();
    let host_port = allocate_port(
        global.port_range_start,
        global.port_range_end,
        &taken,
        &TcpProbe,
    )?;

    // Per-instance config dir.
    let root = state_root
        .join("instances")
        .join(&args.as_name)
        .join("conf");
    std::fs::create_dir_all(&root).map_err(|e| PgForgeError::Io {
        path: root.clone(),
        source: e,
    })?;
    let postgresql_conf = root.join("postgresql.conf");
    let pg_hba = root.join("pg_hba.conf");
    let pgbackrest_conf = root.join("pgbackrest.conf");
    let entrypoint = root.join("clone-entrypoint.sh");
    let pgpass = root.join("pgpass");

    // pg_hba uses the SOURCE's db_name (cloned cluster keeps source's DB).
    std::fs::write(
        &postgresql_conf,
        generate_postgresql_conf(source.instance.preset, plat),
    )
    .map_err(|e| PgForgeError::Io {
        path: postgresql_conf.clone(),
        source: e,
    })?;
    std::fs::write(
        &pg_hba,
        generate_pg_hba(&source.instance.db_name, &source.instance.app_user),
    )
    .map_err(|e| PgForgeError::Io {
        path: pg_hba.clone(),
        source: e,
    })?;
    // pgbackrest.conf: clone gets its OWN repo path for future snapshots.
    std::fs::write(&pgbackrest_conf, generate_pgbackrest_conf(&args.as_name, &s3))
        .map_err(|e| PgForgeError::Io {
            path: pgbackrest_conf.clone(),
            source: e,
        })?;
    std::fs::write(&entrypoint, generate_clone_entrypoint(&source_container))
        .map_err(|e| PgForgeError::Io {
            path: entrypoint.clone(),
            source: e,
        })?;
    std::fs::write(&pgpass, generate_pgpass(&source.instance.pgbackrest_password))
        .map_err(|e| PgForgeError::Io {
            path: pgpass.clone(),
            source: e,
        })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // entrypoint.sh — 0755 executable
        let mut perms = std::fs::metadata(&entrypoint)
            .map_err(|e| PgForgeError::Io {
                path: entrypoint.clone(),
                source: e,
            })?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&entrypoint, perms).map_err(|e| PgForgeError::Io {
            path: entrypoint.clone(),
            source: e,
        })?;
        // .pgpass — 0600 (postgres refuses world-readable)
        let mut pp = std::fs::metadata(&pgpass)
            .map_err(|e| PgForgeError::Io {
                path: pgpass.clone(),
                source: e,
            })?
            .permissions();
        pp.set_mode(0o600);
        std::fs::set_permissions(&pgpass, pp).map_err(|e| PgForgeError::Io {
            path: pgpass.clone(),
            source: e,
        })?;
    }

    docker
        .build_image(&BuildImageSpec {
            tag: format!("pgforge/postgres:{}", source.instance.pg_version),
            dockerfile: dockerfile(source.instance.pg_version),
        })
        .await?;
    docker.ensure_network("pgforge_net").await?;

    let mut env = HashMap::new();
    env.insert("POSTGRES_USER".into(), source.instance.app_user.clone());
    env.insert(
        "POSTGRES_PASSWORD".into(),
        source.instance.app_password.clone(),
    );
    env.insert("POSTGRES_DB".into(), source.instance.db_name.clone());
    env.insert(
        "PGDATA".into(),
        "/var/lib/postgresql/data/pgdata".into(),
    );

    let binds = vec![
        BindMount {
            host_path: postgresql_conf.clone(),
            container_path: "/etc/postgresql/postgresql.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: pg_hba.clone(),
            container_path: "/etc/postgresql/pg_hba.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: pgbackrest_conf.clone(),
            container_path: "/etc/pgbackrest/pgbackrest.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: entrypoint.clone(),
            container_path: "/usr/local/bin/pgforge-clone-entrypoint.sh".into(),
            read_only: true,
        },
        BindMount {
            host_path: pgpass.clone(),
            container_path: "/var/lib/postgresql/.pgpass".into(),
            read_only: true,
        },
    ];
    let volumes = vec![NamedVolume {
        volume_name: format!("pgforge_data_{}", args.as_name),
        container_path: "/var/lib/postgresql/data".into(),
    }];

    let container_name = format!("pgforge_{}", args.as_name);
    let volume_name = format!("pgforge_data_{}", args.as_name);
    let spec = CreateContainerSpec {
        container_name: container_name.clone(),
        image: format!("pgforge/postgres:{}", source.instance.pg_version),
        env,
        binds,
        volumes,
        host_port,
        container_port: 5432,
        memory_mb: tuning.ram_mb,
        network: "pgforge_net".into(),
        shm_size_mb: 256,
        command_override: Some(vec![
            "/usr/local/bin/pgforge-clone-entrypoint.sh".into(),
        ]),
    };
    let id = docker.create_container(&spec).await?;

    let conf_dir = root.clone();
    let result = post_create_steps(docker, &id, &args, source, host_port, &state_root).await;
    match result {
        Ok(state) => Ok(state),
        Err(e) => {
            cleanup_partial(docker, &container_name, &volume_name, &conf_dir).await;
            Err(e)
        }
    }
}

/// Runs all steps after `create_container`: start, wait, persist state.
/// Extracted so the caller can run cleanup_partial on any failure.
async fn post_create_steps<E: DockerEngine>(
    docker: &E,
    id: &str,
    args: &CloneArgs,
    source: InstanceState,
    host_port: u16,
    state_root: &std::path::Path,
) -> Result<InstanceState> {
    docker.start_container(id).await?;
    docker
        .wait_for_container_running(id, std::time::Duration::from_secs(30))
        .await?;
    // pg_basebackup of a small DB takes seconds; larger ones minutes. Allow 10 min.
    wait_for_pg_ready(docker, id, 600).await?;

    let state = InstanceState {
        instance: Instance {
            name: args.as_name.clone(),
            db_name: source.instance.db_name,
            app_user: source.instance.app_user,
            app_password: source.instance.app_password,
            pgbackrest_password: source.instance.pgbackrest_password,
            preset: source.instance.preset,
            pg_version: source.instance.pg_version,
            host_port,
        },
        created_at: crate::time::now_iso(),
    };
    state.save_under(state_root)?;
    Ok(state)
}
