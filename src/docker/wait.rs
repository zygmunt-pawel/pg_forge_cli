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
///
/// Uses a deadline-based loop (via `tokio::time::Instant`) so that the actual
/// wall-clock timeout matches `seconds` regardless of how long each exec takes.
/// Also fails fast (~5s) if the container is consistently not running, which
/// indicates a crash rather than a normal slow startup.
pub async fn wait_for_pg_ready<E: DockerEngine>(
    docker: &E,
    id: &str,
    seconds: u64,
) -> Result<()> {
    let mut consecutive_not_running = 0u32;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match docker
            .exec(id, &["pg_isready", "-h", "/var/run/postgresql"])
            .await
        {
            Ok(out) if out.exit_code == 0 => return Ok(()),
            Ok(_) => {
                consecutive_not_running = 0;
            }
            Err(e) => {
                let msg = e.to_string();
                // Docker exec returns 409 while the container is in
                // restart-loop or hasn't fully transitioned to running.
                // Both resolve themselves within seconds — keep polling.
                if msg.contains("is restarting") || msg.contains("status code 409") {
                    consecutive_not_running = 0;
                } else if msg.contains("is not running") {
                    consecutive_not_running += 1;
                    // Container is stopped (not restart-flapping). After ~5
                    // consecutive reports we conclude it crashed and won't
                    // recover; fail fast rather than running out the deadline.
                    if consecutive_not_running >= 5 {
                        return Err(PgForgeError::Docker(format!(
                            "container {id} is not running (crashed before pg_isready); see container logs"
                        )));
                    }
                } else {
                    return Err(e);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(PgForgeError::Docker(format!(
                "container {id}: postgres did not accept connections within {seconds}s"
            )));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
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
        // Connect as `pgbackrest`, not `postgres`: pgforge's generated
        // pg_hba.conf trusts `pgbackrest` on the local socket but has no
        // entry for the `postgres` role, so `-U postgres` gets "no
        // pg_hba.conf entry" and the wait burns its whole deadline against
        // a healthy, already-promoted postgres. `pgbackrest` always exists
        // on a backup-enabled instance (the only kind restore/clone — the
        // only callers — operate on). `-d postgres` because no `pgbackrest`
        // database exists.
        // `select pg_is_in_recovery()` (no `::text` cast): `psql -tA` renders
        // a raw boolean as `f`/`t`, which is what the `== "f"` check below
        // expects. Casting to ::text would render `false`/`true` and the
        // check would never match.
        let out = docker
            .exec(id, &[
                "psql", "-tA", "-U", "pgbackrest", "-d", "postgres",
                "-h", "/var/run/postgresql",
                "-c", "select pg_is_in_recovery()",
            ])
            .await;
        if let Ok(o) = out && o.exit_code == 0 && o.stdout.trim() == "f" {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(PgForgeError::Docker(format!(
                "container {id}: still in recovery after {seconds}s"
            )));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
