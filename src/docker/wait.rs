use crate::docker::engine::DockerEngine;
use crate::error::{PgForgeError, Result};
use std::time::Duration;

/// Poll `pg_isready -h /var/run/postgresql` inside the container until exit
/// code 0 or `seconds` elapse. Used by create (30s) and clone/restore (600s).
///
/// Tolerates the Docker 409 "Container is restarting" error: a container
/// that crashes during boot and gets re-launched by `--restart=unless-stopped`
/// briefly refuses exec calls. We treat that the same as "pg not ready yet"
/// and retry — important on restore where pgbackrest can take a while and
/// the postgres entrypoint may flap once before settling.
pub async fn wait_for_pg_ready<E: DockerEngine>(
    docker: &E,
    id: &str,
    seconds: u64,
) -> Result<()> {
    for _ in 0..seconds {
        match docker
            .exec(id, &["pg_isready", "-h", "/var/run/postgresql"])
            .await
        {
            Ok(out) if out.exit_code == 0 => return Ok(()),
            Ok(_) => {} // pg not ready, keep waiting
            Err(e) => {
                let msg = e.to_string();
                // Docker exec returns 409 while the container is in
                // restart-loop or hasn't fully transitioned to running.
                // Both resolve themselves within seconds — keep polling.
                let transient = msg.contains("is restarting")
                    || msg.contains("is not running")
                    || msg.contains("status code 409");
                if !transient {
                    return Err(e);
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(PgForgeError::Docker(format!(
        "container {id}: postgres did not accept connections within {seconds}s"
    )))
}
