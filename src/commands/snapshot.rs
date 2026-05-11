use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::domain::snapshot::{SnapshotKind, SnapshotRecord};
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::parse::parse_backup_label;
use crate::state::instance::InstanceState;
use crate::state::snapshots::SnapshotsFile;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SnapshotArgs {
    pub instance: String,
    pub user_label: Option<String>,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: SnapshotArgs) -> Result<SnapshotRecord> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let _ = InstanceState::load_under(&state_root, &args.instance)?; // ensures instance exists
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: SnapshotArgs,
    docker: &E,
    state_root: PathBuf,
) -> Result<SnapshotRecord> {
    let container = format!("pgforge_{}", args.instance);
    if !docker.container_running(&container).await? {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "container {container:?} is not running. Start it with `docker start {container}` and retry."
        )));
    }

    let out = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                "pgbackrest --stanza=main --type=full backup",
            ],
        )
        .await?;
    if out.exit_code != 0 {
        return Err(PgForgeError::Docker(format!(
            "pgbackrest backup failed (exit {}): {}",
            out.exit_code, out.stderr
        )));
    }
    let label = parse_backup_label(&out.stdout).ok_or_else(|| {
        PgForgeError::Anyhow(anyhow::anyhow!(
            "pgbackrest backup succeeded but no label found in stdout — output: {}",
            out.stdout
        ))
    })?;

    let mut file = SnapshotsFile::load_for(&state_root, &args.instance)?;
    let record = SnapshotRecord {
        label: label.clone(),
        kind: SnapshotKind::Full,
        user_label: args.user_label,
        taken_at: crate::time::now_iso(),
    };
    file.snapshots.push(record.clone());
    file.save_for(&state_root, &args.instance)?;

    Ok(record)
}
