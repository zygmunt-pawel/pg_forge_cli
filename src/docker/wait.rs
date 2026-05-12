use crate::docker::engine::DockerEngine;
use crate::error::{PgForgeError, Result};
use std::time::Duration;
use tokio::time::Instant;

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

/// After `wait_for_pg_ready` returns, the cluster may still be in recovery
/// (during `target-action=promote` Postgres briefly accepts connections
/// before timeline switch). Poll `pg_is_in_recovery()` until it returns
/// `false`, or fail after `seconds`.
pub async fn wait_for_recovery_end<E: DockerEngine>(
    docker: &E,
    id: &str,
    seconds: u64,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let out = docker
            .exec(id, &[
                "psql", "-tA", "-U", "postgres", "-h", "/var/run/postgresql",
                "-c", "select pg_is_in_recovery()::text",
            ])
            .await;
        if let Ok(o) = out {
            if o.exit_code == 0 && o.stdout.trim() == "f" {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(PgForgeError::Docker(format!(
                "container {id}: still in recovery after {seconds}s"
            )));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
