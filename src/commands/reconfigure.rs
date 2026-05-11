use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::error::{PgForgeError, Result};
use crate::postgres::hba::generate_pg_hba;
use crate::state::instance::InstanceState;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ReconfigureArgs {
    pub instance: String,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: ReconfigureArgs) -> Result<()> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let state = InstanceState::load_under(&state_root, &args.instance)?;
    let docker = BollardEngine::connect()?;
    run_with_engine(args.instance.clone(), state, &docker, state_root).await
}

pub async fn run_with_engine<E: DockerEngine>(
    instance: String,
    state: InstanceState,
    docker: &E,
    state_root: PathBuf,
) -> Result<()> {
    // 1. Regenerate pg_hba.conf on host (bind-mounted into container).
    let conf_dir = state_root
        .join("instances")
        .join(&instance)
        .join("conf");
    let pg_hba_path = conf_dir.join("pg_hba.conf");
    let new_hba = generate_pg_hba(&state.instance.db_name, &state.instance.app_user);
    std::fs::write(&pg_hba_path, new_hba).map_err(|e| PgForgeError::Io {
        path: pg_hba_path.clone(),
        source: e,
    })?;

    // 2. Reload PG inside container — picks up new pg_hba without restart.
    let container = format!("pgforge_{}", instance);
    let out = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                "pg_ctl reload -D /var/lib/postgresql/data/pgdata",
            ],
        )
        .await?;
    if out.exit_code != 0 {
        return Err(PgForgeError::Docker(format!(
            "pg_ctl reload failed (exit {}): {}",
            out.exit_code, out.stderr
        )));
    }
    Ok(())
}
