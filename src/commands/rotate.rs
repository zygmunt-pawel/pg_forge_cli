//! `pgforge rotate <name>` — recreate the container for an existing instance,
//! keeping its volume (data) intact. Regenerates configs from the current
//! pgforge code so pre-fix instances pick up hardening + the bind-mount fix
//! without losing their data.
//!
//! Workflow:
//!   1. Load state.toml.
//!   2. docker stop (10s grace) + docker rm (volume retained).
//!   3. Regenerate conf/ files (postgresql.conf, pg_hba.conf, pgbackrest.conf,
//!      init_sql) from the current generators + the instance's recorded args.
//!   4. Re-build the image (idempotent) and create a fresh container on the
//!      SAME named volume, SAME host_port, with current cmd_override flags.
//!   5. Start, wait_for_pg_ready.
//!   6. For backup_enabled instances, ensure the `pgreplica` role exists
//!      (instances created before Plan 3.5 don't have it; initdb hooks only
//!      run on empty PGDATA so we can't recreate the role that way).

use crate::commands::create::ConfigLayout;
use crate::config::global::GlobalConfig;
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::{
    BindMount, BuildImageSpec, CreateContainerSpec, DockerEngine, NamedVolume,
};
use crate::docker::image::dockerfile;
use crate::domain::instance::Instance;
use crate::domain::platform::current_platform;
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::generate_pgbackrest_conf;
use crate::postgres::conf::generate_postgresql_conf_with_archive;
use crate::postgres::hba::generate_pg_hba;
use crate::postgres::init_sql::generate_init_sql;
use crate::state::instance::InstanceState;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RotateArgs {
    pub name: String,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: RotateArgs) -> Result<()> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let global = GlobalConfig::load()?;
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root, global).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: RotateArgs,
    docker: &E,
    state_root: PathBuf,
    global: GlobalConfig,
) -> Result<()> {
    Instance::validate_name(&args.name)?;
    let state = InstanceState::load_under(&state_root, &args.name)?;
    let instance = &state.instance;
    let plat = current_platform();
    let tuning = instance.preset.tuning();

    let container_name = format!("pgforge_{}", instance.name);
    let volume_name = format!("pgforge_data_{}", instance.name);

    // 1. Stop + remove container (volume stays).
    if docker.container_running(&container_name).await? {
        docker.stop_container(&container_name).await?;
    }
    if docker.container_exists(&container_name).await? {
        docker.remove_container(&container_name, true).await?;
    }

    // 2. Regenerate configs.
    let s3 = if instance.backup_enabled {
        Some(global.s3.clone().ok_or_else(|| {
            PgForgeError::Anyhow(anyhow::anyhow!(
                "instance {:?} is backup-enabled but S3 settings are missing from global config.",
                instance.name
            ))
        })?)
    } else {
        None
    };

    let layout = ConfigLayout::for_instance(&state_root, &instance.name);
    crate::util::fs::create_secret_dir(&layout.root)?;
    let with_archive = instance.backup_enabled;
    std::fs::write(
        &layout.postgresql_conf,
        generate_postgresql_conf_with_archive(instance.preset, plat, with_archive),
    )
    .map_err(|e| PgForgeError::Io {
        path: layout.postgresql_conf.clone(),
        source: e,
    })?;
    std::fs::write(
        &layout.pg_hba,
        generate_pg_hba(&instance.db_name, &instance.app_user),
    )
    .map_err(|e| PgForgeError::Io {
        path: layout.pg_hba.clone(),
        source: e,
    })?;
    if let Some(s3) = s3.as_ref() {
        crate::util::fs::write_secret(
            &layout.pgbackrest_conf,
            generate_pgbackrest_conf(&instance.name, s3),
        )?;
        let init_dir = layout.init_sql.parent().unwrap().to_path_buf();
        crate::util::fs::create_secret_dir(&init_dir)?;
        // init_sql isn't read on rotate (PGDATA is not empty so initdb
        // hooks don't run), but we keep the file in sync so a future
        // delete+restore-from-snapshot picks up the current contents.
        crate::util::fs::write_secret(
            &layout.init_sql,
            generate_init_sql(&instance.pgbackrest_password),
        )?;
    }

    // 3. Image + network (idempotent).
    docker
        .build_image(&BuildImageSpec {
            tag: format!("pgforge/postgres:{}", instance.pg_version),
            dockerfile: dockerfile(instance.pg_version),
        })
        .await?;
    docker.ensure_network("pgforge_net").await?;

    // 4. Create container with SAME volume + SAME port.
    let mut env = HashMap::new();
    env.insert("POSTGRES_USER".into(), instance.app_user.clone());
    env.insert("POSTGRES_PASSWORD".into(), instance.app_password.clone());
    env.insert("POSTGRES_DB".into(), instance.db_name.clone());
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
    if instance.backup_enabled {
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
        volume_name: volume_name.clone(),
        container_path: "/var/lib/postgresql/data".into(),
    }];

    let spec = CreateContainerSpec {
        container_name: container_name.clone(),
        image: format!("pgforge/postgres:{}", instance.pg_version),
        env,
        binds,
        volumes,
        host_port: instance.host_port,
        container_port: 5432,
        memory_mb: tuning.ram_mb,
        network: "pgforge_net".into(),
        shm_size_mb: 256,
        entrypoint_override: None,
        cmd_override: Some(vec![
            "postgres".into(),
            "-c".into(),
            "config_file=/etc/postgresql/postgresql.conf".into(),
            "-c".into(),
            "hba_file=/etc/postgresql/pg_hba.conf".into(),
        ]),
    };
    let id = docker.create_container(&spec).await?;
    docker.start_container(&id).await?;
    docker
        .wait_for_container_running(&id, std::time::Duration::from_secs(30))
        .await?;
    crate::docker::wait::wait_for_pg_ready(docker, &id, 60).await?;

    // 5. Ensure pgreplica role exists for backup-enabled instances. Instances
    // created before Plan 3.5 only have `pgbackrest` — `pgforge clone` needs
    // a non-SUPERUSER pgreplica role exposed via host replication. CREATE
    // ROLE IF NOT EXISTS isn't supported for roles, so use a DO block.
    if instance.backup_enabled {
        let escaped = instance.pgbackrest_password.replace('\'', "''");
        let sql = format!(
            "DO $$ BEGIN \
             IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pgreplica') THEN \
               CREATE ROLE pgreplica WITH LOGIN REPLICATION PASSWORD '{escaped}'; \
             END IF; \
             END $$;"
        );
        let out = docker
            .exec(&id, &["su", "-", "postgres", "-c", &format!("psql -c \"{sql}\"")])
            .await?;
        if out.exit_code != 0 {
            return Err(PgForgeError::Docker(format!(
                "ensure-pgreplica failed (exit {}): stdout={:?} stderr={:?}",
                out.exit_code, out.stdout, out.stderr
            )));
        }
    }

    Ok(())
}
