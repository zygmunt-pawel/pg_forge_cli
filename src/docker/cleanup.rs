use crate::docker::engine::DockerEngine;
use std::path::Path;

/// Best-effort cleanup of a half-created instance. Used after a mid-flight
/// failure (between create_container and state.save_under). Each step is
/// independent and emits a tracing warning on failure — the caller already
/// has a primary error to surface, but a silent rollback leak would let
/// orphan resources accumulate and (for the conf dir) leave plaintext
/// secrets — .pgpass, init_sql, pgbackrest.conf with S3 keys — on disk.
pub async fn cleanup_partial<E: DockerEngine>(
    docker: &E,
    container_name: &str,
    volume_name: &str,
    conf_dir: &Path,
) {
    if let Err(e) = docker.remove_container(container_name, true).await {
        tracing::warn!(
            target: "pgforge::cleanup",
            "cleanup: remove_container({container_name}) failed: {e}"
        );
    }
    if let Err(e) = docker.remove_volume(volume_name).await {
        tracing::warn!(
            target: "pgforge::cleanup",
            "cleanup: remove_volume({volume_name}) failed: {e}"
        );
    }
    // Conf dir holds .pgpass + init_sql with role password + pgbackrest.conf
    // with S3 keys. Failing to remove it leaks credentials.
    if conf_dir.exists() && let Err(e) = std::fs::remove_dir_all(conf_dir) {
        tracing::warn!(
            target: "pgforge::cleanup",
            "cleanup: remove_dir_all({}) failed: {e} — credentials may remain on disk",
            conf_dir.display()
        );
    }
}
