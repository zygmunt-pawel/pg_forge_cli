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
}

#[derive(Debug, Clone)]
pub struct NamedVolume {
    pub volume_name: String,
    pub container_path: String,
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
}
