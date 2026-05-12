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
    if !source.instance.backup_enabled {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "source instance {:?} was created with --no-backup; it has no \
             pgbackrest stanza, so the cloned instance couldn't archive WAL \
             either. Recreate the source with backups enabled first.",
            args.source
        )));
    }
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

    // Per-instance config dir. 0700 — contains pgbackrest.conf (S3 keys) and
    // pgpass (replication password).
    let root = state_root
        .join("instances")
        .join(&args.as_name)
        .join("conf");
    crate::util::fs::create_secret_dir(&root)?;
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
    // Secret — carries S3 access_key + secret_key.
    crate::util::fs::write_secret(&pgbackrest_conf, generate_pgbackrest_conf(&args.as_name, &s3, 30))?;
    std::fs::write(&entrypoint, generate_clone_entrypoint(&source_container))
        .map_err(|e| PgForgeError::Io {
            path: entrypoint.clone(),
            source: e,
        })?;
    // .pgpass — write_secret sets 0600 (libpq refuses world-readable).
    crate::util::fs::write_secret(&pgpass, generate_pgpass(&source.instance.pgbackrest_password))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // entrypoint.sh — 0755 executable (NOT a secret — bind-mounted as
        // the container's entrypoint, no credentials inside).
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
        // Clone needs to pg_basebackup BEFORE postgres starts; our entrypoint
        // wraps that work and then `exec docker-entrypoint.sh postgres`. The
        // chained docker-entrypoint then needs the same -c flags as `create`
        // so the cloned cluster picks up bind-mounted configs (pg_hba in
        // particular — without it, `pgforge clone <of-this-clone>` would
        // fail because no host-replication rule is in effect).
        entrypoint_override: Some(vec![
            "/usr/local/bin/pgforge-clone-entrypoint.sh".into(),
        ]),
        cmd_override: None,
    };
    let id = docker.create_container(&spec).await?;

    let conf_dir = root.clone();
    let state = match bootstrap_clone(docker, &id, &args, source, host_port).await {
        Ok(state) => state,
        Err(e) => {
            cleanup_partial(docker, &container_name, &volume_name, &conf_dir).await;
            return Err(e);
        }
    };

    // Saving state.toml happens OUTSIDE the cleanup wrap. The container is
    // fully bootstrapped and healthy — pg_basebackup completed and the
    // stanza is created. If writing the local state file fails (disk full,
    // bad perms), destroying the working clone would be a worse outcome
    // than the missing state.toml — the user has the actual instance and
    // can re-save it. Log loudly so they know.
    if let Err(e) = state.save_under(&state_root) {
        tracing::error!(
            target: "pgforge::clone",
            "clone bootstrapped successfully but state.toml save failed: {e}. \
             Container {container_name} is running on port {host_port}; \
             rerun `pgforge clone --source {} --as {}` after fixing the local \
             filesystem, or save state.toml manually under {state_root:?}.",
            args.source, args.as_name
        );
        return Err(e);
    }
    Ok(state)
}

/// All steps after `create_container` that must rollback on failure: start,
/// wait for pg ready, create pgbackrest stanza. Returns the in-memory state
/// — saving it to disk is the caller's responsibility (and is kept OUTSIDE
/// the cleanup wrap so a healthy container survives a state-save error).
async fn bootstrap_clone<E: DockerEngine>(
    docker: &E,
    id: &str,
    args: &CloneArgs,
    source: InstanceState,
    host_port: u16,
) -> Result<InstanceState> {
    docker.start_container(id).await?;
    docker
        .wait_for_container_running(id, std::time::Duration::from_secs(30))
        .await?;
    // pg_basebackup of a small DB takes seconds; larger ones minutes. Allow 10 min.
    wait_for_pg_ready(docker, id, 600).await?;

    // Create the pgbackrest stanza on the clone's OWN repo path so archive_command
    // can begin pushing WAL. The cloned cluster has a new system identifier vs.
    // source, so a fresh stanza on `/pgforge/<as_name>/` is what we want — never
    // reuse the source's stanza, that would mix WAL from two diverged timelines.
    let stanza = docker
        .exec(
            id,
            &[
                "su", "-", "postgres", "-c",
                "pgbackrest --stanza=main stanza-create",
            ],
        )
        .await?;
    if stanza.exit_code != 0 {
        return Err(PgForgeError::Docker(format!(
            "pgbackrest stanza-create failed on clone (exit {}): stdout={:?} stderr={:?}",
            stanza.exit_code, stanza.stdout, stanza.stderr
        )));
    }

    Ok(InstanceState {
        instance: Instance {
            name: args.as_name.clone(),
            db_name: source.instance.db_name,
            app_user: source.instance.app_user,
            app_password: source.instance.app_password,
            pgbackrest_password: source.instance.pgbackrest_password,
            preset: source.instance.preset,
            pg_version: source.instance.pg_version,
            host_port,
            // A clone of a backup-enabled source is itself backup-enabled
            // (clone.rs configures its own pgbackrest stanza in
            // bootstrap_clone). Source must be backup-enabled to clone at
            // all — guarded earlier in run_with_engine.
            backup_enabled: true,
            volume_name_override: None,
            retain_days: source.instance.retain_days,
            snapshot_hour: source.instance.snapshot_hour,
            last_snapshot_at: None,
        },
        created_at: crate::time::now_iso(),
    })
}
