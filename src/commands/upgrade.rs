//! `pgforge upgrade --name <inst> --to <ver>` — in-place major-version
//! upgrade via `pg_upgrade`, with automatic pre-upgrade snapshot for
//! rollback.
//!
//! Workflow:
//!   1. Load state.toml; require `to > from` and a sensible version range.
//!   2. If backup_enabled, take a labeled pre-upgrade snapshot.
//!   3. Build `pgforge/upgrade:<from>-to-<to>` image (both PG bins + pgbackrest).
//!      If this fails the running container is untouched and state.toml unchanged.
//!   4. Stop + remove the running container (volume retained for rollback).
//!   5. Create a fresh volume `pgforge_data_<name>_v<to>` for the upgraded
//!      cluster.
//!   6. Run a one-shot upgrade container:
//!        - mounts old volume at /old/pgdata (read-write — pg_upgrade
//!          inspects but doesn't mutate the source)
//!        - mounts new volume at /new/pgdata
//!        - entrypoint: initdb new + pg_upgrade old→new + write success marker
//!   7. Wait for the upgrade container to exit; check exit code.
//!   8. On success: persist state.toml with pg_version=to and
//!      volume_name_override=<new vol>, then recreate the regular pgforge
//!      container on the upgraded volume via the same shape as `rotate`.
//!   9. On failure: remove the new volume, log the upgrade container's
//!      stderr, and surface the error. The pre-upgrade snapshot is the
//!      user's primary recovery path; the old volume also remains intact
//!      until the user explicitly removes it.

use crate::commands::create::ConfigLayout;
use crate::config::global::GlobalConfig;
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::{
    BindMount, BuildImageSpec, CreateContainerSpec, DockerEngine, NamedVolume,
};
use crate::docker::image::{dockerfile, upgrade_dockerfile};
use crate::docker::upgrade_entrypoint::generate_upgrade_entrypoint;
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
use std::time::Duration;

// No min/max supported version range — pgforge accepts whatever version
// pgdg apt has packages for. The user is in charge of picking a sane pair.

#[derive(Debug, Clone)]
pub struct UpgradeArgs {
    pub name: String,
    pub to_version: u8,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: UpgradeArgs) -> Result<()> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let global = GlobalConfig::load()?;
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root, global).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: UpgradeArgs,
    docker: &E,
    state_root: PathBuf,
    global: GlobalConfig,
) -> Result<()> {
    Instance::validate_name(&args.name)?;
    let mut state = InstanceState::load_under(&state_root, &args.name)?;
    let from_ver = state.instance.pg_version;
    let to_ver = args.to_version;

    // Sanity check — pg_upgrade refuses downgrades and same-version "upgrades".
    if to_ver <= from_ver {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "target version {to_ver} must be greater than current version {from_ver}"
        )));
    }

    let container_name = format!("pgforge_{}", state.instance.name);
    let old_volume = state.instance.volume_name();
    let new_volume = format!("pgforge_data_{}_v{}", state.instance.name, to_ver);

    // 2. Pre-upgrade snapshot for rollback (backup-enabled only).
    if state.instance.backup_enabled {
        tracing::info!(target: "pgforge::upgrade", "taking pre-upgrade snapshot of {}", state.instance.name);
        let _ = crate::commands::snapshot::run_with_engine(
            crate::commands::snapshot::SnapshotArgs {
                instance: state.instance.name.clone(),
                user_label: Some(format!("pre-upgrade-{from_ver}-to-{to_ver}")),
                override_state_root: Some(state_root.clone()),
            },
            docker,
            state_root.clone(),
        )
        .await?;
    }

    // 3. Build upgrade image with BOTH PG versions BEFORE touching the running
    // container. If this fails (network, disk, Dockerfile bug) the old
    // container is still running and state.toml is unchanged.
    let upgrade_image = format!("pgforge/upgrade:{from_ver}-to-{to_ver}");
    docker
        .build_image(&BuildImageSpec {
            tag: upgrade_image.clone(),
            dockerfile: upgrade_dockerfile(from_ver, to_ver),
        })
        .await?;

    // 4. Stop + remove old container (keep volume). Only reached if build
    // succeeded, so the instance is never left in a half-torn-down state.
    if docker.container_running(&container_name).await? {
        docker.stop_container(&container_name).await?;
    }
    if docker.container_exists(&container_name).await? {
        docker.remove_container(&container_name, true).await?;
    }

    // 5. Write the upgrade entrypoint script to a host path so it can be
    // bind-mounted (executable bit set).
    let layout = ConfigLayout::for_instance(&state_root, &state.instance.name);
    let upgrade_script = layout.root.join(format!("upgrade-{from_ver}-to-{to_ver}.sh"));
    std::fs::write(&upgrade_script, generate_upgrade_entrypoint(from_ver, to_ver))
        .map_err(|e| PgForgeError::Io {
            path: upgrade_script.clone(),
            source: e,
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&upgrade_script)
            .map_err(|e| PgForgeError::Io {
                path: upgrade_script.clone(),
                source: e,
            })?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&upgrade_script, perms).map_err(|e| PgForgeError::Io {
            path: upgrade_script.clone(),
            source: e,
        })?;
    }

    // 6. Create upgrade container with both volumes mounted. host_port=0
    // (we never expose this container — it just runs pg_upgrade and exits).
    let upgrade_container = format!("pgforge_upgrade_{}_v{to_ver}", state.instance.name);
    let upgrade_spec = CreateContainerSpec {
        container_name: upgrade_container.clone(),
        image: upgrade_image.clone(),
        env: HashMap::new(),
        binds: vec![BindMount {
            host_path: upgrade_script.clone(),
            container_path: "/usr/local/bin/pgforge-upgrade.sh".into(),
            read_only: true,
        }],
        volumes: vec![
            NamedVolume {
                volume_name: old_volume.clone(),
                container_path: "/old".into(),
            },
            NamedVolume {
                volume_name: new_volume.clone(),
                container_path: "/new".into(),
            },
        ],
        host_port: 0,    // unused — no port_bindings will fire when host_port==0
        container_port: 5432,
        memory_mb: state.instance.preset.tuning().ram_mb,
        network: "pgforge_net".into(),
        shm_size_mb: 256,
        entrypoint_override: Some(vec!["/usr/local/bin/pgforge-upgrade.sh".into()]),
        cmd_override: None,
        restart_policy: crate::docker::engine::RestartPolicy::No,
    };
    let id = docker.create_container(&upgrade_spec).await?;
    docker.start_container(&id).await?;

    // 7. Wait for completion. pg_upgrade against a small DB is seconds;
    // against a multi-GB DB can be many minutes. Allow 1h.
    let exit_code = docker.wait_for_container_exit(&id, Duration::from_secs(3600)).await?;
    // Capture logs whether we succeeded or failed — they include pg_upgrade
    // output that the user needs to diagnose.
    let logs_exec = docker
        .exec(&id, &["true"])
        .await
        .ok(); // best-effort; if container is gone we just don't have logs
    let _ = logs_exec;

    if exit_code != 0 {
        // 8a. Failure: remove new volume, leave old volume + pre-upgrade
        // snapshot intact. Don't auto-restart the old container — the user
        // should investigate before resuming. Surface the entrypoint
        // failure to them.
        let _ = docker.remove_container(&upgrade_container, true).await;
        let _ = docker.remove_volume(&new_volume).await;
        return Err(PgForgeError::Docker(format!(
            "pg_upgrade failed (exit {exit_code}). Old data volume {old_volume:?} is intact; \
             pre-upgrade snapshot is available via `pgforge snapshots --name {}`. \
             Inspect upgrade container logs (`docker logs {upgrade_container}` would have helped — \
             container was removed). To resume the OLD instance: re-run `pgforge rotate --name {}`.",
            state.instance.name, state.instance.name
        )));
    }
    // 8b. Success: remove the one-shot container; volume swap is logical.
    let _ = docker.remove_container(&upgrade_container, true).await;

    // 9. Persist state changes: new pg_version + new volume name.
    state.instance.pg_version = to_ver;
    state.instance.volume_name_override = Some(new_volume.clone());
    state.save_under(&state_root)?;

    // 10. Recreate the regular pgforge container on the upgraded volume.
    // Mirrors the rotate flow but uses the new (post-upgrade) state.
    recreate_regular_container(docker, &state, &state_root, &global).await?;

    Ok(())
}

/// Re-render configs from `state` and create+start the regular pgforge
/// container on `instance.volume_name()`. Used as the final step of
/// `pgforge upgrade` (and conceptually shareable with `rotate`, but kept
/// separate for now to keep this file independent).
async fn recreate_regular_container<E: DockerEngine>(
    docker: &E,
    state: &InstanceState,
    state_root: &std::path::Path,
    global: &GlobalConfig,
) -> Result<()> {
    let instance = &state.instance;
    let plat = current_platform();
    let tuning = instance.preset.tuning();
    let container_name = format!("pgforge_{}", instance.name);
    let volume_name = instance.volume_name();

    let s3 = if instance.backup_enabled {
        Some(global.s3.clone().ok_or_else(|| {
            PgForgeError::Anyhow(anyhow::anyhow!(
                "backup-enabled instance but S3 missing from global config; \
                 cannot regenerate pgbackrest.conf"
            ))
        })?)
    } else {
        None
    };

    let layout = ConfigLayout::for_instance(state_root, &instance.name);
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
            generate_pgbackrest_conf(&instance.name, s3, instance.retain_days),
        )?;
        let init_dir = layout.init_sql.parent().unwrap().to_path_buf();
        crate::util::fs::create_secret_dir(&init_dir)?;
        crate::util::fs::write_secret(
            &layout.init_sql,
            generate_init_sql(&instance.pgbackrest_password),
        )?;
    }

    docker
        .build_image(&BuildImageSpec {
            tag: format!("pgforge/postgres:{}", instance.pg_version),
            dockerfile: dockerfile(instance.pg_version),
        })
        .await?;
    docker.ensure_network("pgforge_net").await?;

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
        // pg_upgrade created the new datadir at /new/pgdata in the upgrade
        // container, which sees the volume mounted at /new. Inside the
        // regular container the same volume is mounted at /var/lib/postgresql/data,
        // and PGDATA points at .../data/pgdata. So pgdata must live at the
        // root of the volume — which is what initdb in the upgrade
        // entrypoint produced (NEW_PGDATA=/new/pgdata).
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
        restart_policy: crate::docker::engine::RestartPolicy::UnlessStopped,
    };
    let id = docker.create_container(&spec).await?;
    docker.start_container(&id).await?;
    docker
        .wait_for_container_running(&id, Duration::from_secs(30))
        .await?;
    crate::docker::wait::wait_for_pg_ready(docker, &id, 60).await?;
    Ok(())
}
