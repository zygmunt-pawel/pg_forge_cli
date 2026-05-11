use crate::config::global::GlobalConfig;
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::{
    BindMount, BuildImageSpec, CreateContainerSpec, DockerEngine, NamedVolume,
};
use crate::docker::image::dockerfile;
use crate::docker::restore_entrypoint::generate_restore_entrypoint;
use crate::domain::instance::Instance;
use crate::domain::platform::current_platform;
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::generate_pgbackrest_conf;
use crate::ports::{TcpProbe, allocate_port};
use crate::postgres::conf::generate_postgresql_conf;
use crate::postgres::hba::generate_pg_hba;
use crate::state::instance::InstanceState;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RestoreArgs {
    pub source: String,
    pub as_name: String,
    pub target_time: Option<String>,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: RestoreArgs) -> Result<InstanceState> {
    Instance::validate_name(&args.as_name)?;
    if let Some(t) = &args.target_time {
        // Validate user-supplied time before doing any work.
        crate::time::parse_target_time(t)?;
    }
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

    // Source must exist; new name must NOT.
    let source = InstanceState::load_under(&state_root, &args.source)?;
    if InstanceState::exists_under(&state_root, &args.as_name) {
        return Err(PgForgeError::InstanceExists(args.as_name.clone()));
    }

    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root, global, s3, source).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: RestoreArgs,
    docker: &E,
    state_root: PathBuf,
    global: GlobalConfig,
    s3: crate::pgbackrest::conf::S3Settings,
    source: InstanceState,
) -> Result<InstanceState> {
    let plat = current_platform();
    let tuning = source.instance.preset.tuning();

    // 1. Allocate a port — avoid all known instances.
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

    // 2. Per-instance config dir for the NEW restored instance.
    // pgbackrest.conf uses the SOURCE name so the repo path matches.
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
    let entrypoint = root.join("restore-entrypoint.sh");

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
    std::fs::write(
        &pgbackrest_conf,
        generate_pgbackrest_conf(&args.source, &s3),
    )
    .map_err(|e| PgForgeError::Io {
        path: pgbackrest_conf.clone(),
        source: e,
    })?;
    std::fs::write(
        &entrypoint,
        generate_restore_entrypoint(args.target_time.as_deref()),
    )
    .map_err(|e| PgForgeError::Io {
        path: entrypoint.clone(),
        source: e,
    })?;

    // Make script executable on unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
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
    }

    // 3. Image (same as source's PG version).
    docker
        .build_image(&BuildImageSpec {
            tag: format!("pgforge/postgres:{}", source.instance.pg_version),
            dockerfile: dockerfile(source.instance.pg_version),
        })
        .await?;
    docker.ensure_network("pgforge_net").await?;

    // 4. Container with command_override = our entrypoint. No init SQL —
    // PGDATA gets populated by pgbackrest restore, so initdb doesn't run.
    let mut env = HashMap::new();
    env.insert("POSTGRES_USER".into(), source.instance.app_user.clone());
    env.insert(
        "POSTGRES_PASSWORD".into(),
        source.instance.app_password.clone(),
    );
    env.insert("POSTGRES_DB".into(), args.as_name.clone());
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
            container_path: "/usr/local/bin/pgforge-restore-entrypoint.sh".into(),
            read_only: true,
        },
    ];
    let volumes = vec![NamedVolume {
        volume_name: format!("pgforge_data_{}", args.as_name),
        container_path: "/var/lib/postgresql/data".into(),
    }];

    let spec = CreateContainerSpec {
        container_name: format!("pgforge_{}", args.as_name),
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
            "/usr/local/bin/pgforge-restore-entrypoint.sh".into(),
        ]),
    };
    let id = docker.create_container(&spec).await?;
    docker.start_container(&id).await?;
    docker
        .wait_for_container_running(&id, std::time::Duration::from_secs(30))
        .await?;
    wait_for_pg_ready_long(docker, &id).await?;

    // 5. Persist state for the new instance.
    let state = InstanceState {
        instance: Instance {
            name: args.as_name.clone(),
            db_name: source.instance.db_name.clone(),
            app_user: source.instance.app_user,
            app_password: source.instance.app_password,
            pgbackrest_password: source.instance.pgbackrest_password,
            preset: source.instance.preset,
            pg_version: source.instance.pg_version,
            host_port,
        },
        created_at: crate::time::now_iso(),
    };
    state.save_under(&state_root)?;
    Ok(state)
}

async fn wait_for_pg_ready_long<E: DockerEngine>(docker: &E, id: &str) -> Result<()> {
    for _ in 0..600 {
        let out = docker
            .exec(id, &["pg_isready", "-h", "/var/run/postgresql"])
            .await?;
        if out.exit_code == 0 {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err(PgForgeError::Docker(format!(
        "container {id}: restored postgres did not accept connections within 10 minutes"
    )))
}
