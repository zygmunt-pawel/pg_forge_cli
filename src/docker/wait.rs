use crate::docker::engine::DockerEngine;
use crate::error::{PgForgeError, Result};
use std::time::Duration;

/// Poll `pg_isready -h /var/run/postgresql` inside the container until exit
/// code 0 or `seconds` elapse. Used by create (30s) and clone/restore (600s).
pub async fn wait_for_pg_ready<E: DockerEngine>(
    docker: &E,
    id: &str,
    seconds: u64,
) -> Result<()> {
    for _ in 0..seconds {
        let out = docker
            .exec(id, &["pg_isready", "-h", "/var/run/postgresql"])
            .await?;
        if out.exit_code == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(PgForgeError::Docker(format!(
        "container {id}: postgres did not accept connections within {seconds}s"
    )))
}
