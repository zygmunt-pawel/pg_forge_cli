use crate::error::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct BindMount {
    pub host_path: PathBuf,
    pub container_path: String,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct CreateContainerSpec {
    pub container_name: String,
    pub image: String,
    pub env: HashMap<String, String>,
    pub binds: Vec<BindMount>,
    pub volumes: Vec<NamedVolume>,
    pub host_port: u16,
    pub container_port: u16,
    pub memory_mb: u32,
    pub network: String,
    pub shm_size_mb: u32,
    /// Override the container's default entrypoint/command. None = use image default.
    pub command_override: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct NamedVolume {
    pub volume_name: String,
    pub container_path: String,
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
}

#[derive(Debug, Clone)]
pub struct BuildImageSpec {
    /// Tag the resulting image will be saved under, e.g. "pgforge/postgres:18".
    pub tag: String,
    /// Dockerfile contents.
    pub dockerfile: String,
}

#[async_trait]
pub trait DockerEngine: Send + Sync {
    async fn build_image(&self, spec: &BuildImageSpec) -> Result<()>;
    async fn ensure_network(&self, name: &str) -> Result<()>;
    async fn create_container(&self, spec: &CreateContainerSpec) -> Result<String>;
    async fn start_container(&self, id: &str) -> Result<()>;
    async fn container_exists(&self, name: &str) -> Result<bool>;

    /// True iff the container exists AND `inspect.state.running == true`.
    /// Distinct from `container_exists`, which also returns true for stopped
    /// containers — the wrong semantics when callers need "can I exec / connect".
    async fn container_running(&self, name: &str) -> Result<bool>;

    /// Run a command inside a running container. Returns combined output.
    async fn exec(&self, id: &str, cmd: &[&str]) -> Result<ExecOutput>;

    /// Stop a running container (SIGTERM, grace period 10s, then SIGKILL).
    async fn stop_container(&self, id: &str) -> Result<()>;

    /// Block until inspect reports State.Running == true, or `timeout` elapses.
    async fn wait_for_container_running(
        &self,
        id: &str,
        timeout: std::time::Duration,
    ) -> Result<()>;

    /// Remove a container (force=true → kill if running). Used for rollback.
    async fn remove_container(&self, id: &str, force: bool) -> Result<()>;

    /// Remove a named volume. Idempotent: no-op if the volume doesn't exist.
    async fn remove_volume(&self, name: &str) -> Result<()>;
}
