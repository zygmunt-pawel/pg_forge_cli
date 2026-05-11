use crate::docker::engine::DockerEngine;

/// Best-effort cleanup of a half-created instance. Used after a mid-flight
/// failure (between create_container and state.save_under). Swallows
/// errors from individual steps — the caller already has a primary error
/// they want to surface.
pub async fn cleanup_partial<E: DockerEngine>(
    docker: &E,
    container_name: &str,
    volume_name: &str,
) {
    let _ = docker.remove_container(container_name, true).await;
    let _ = docker.remove_volume(volume_name).await;
}
