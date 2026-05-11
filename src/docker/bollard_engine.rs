use crate::docker::engine::{BuildImageSpec, CreateContainerSpec, DockerEngine};
use crate::error::Result;
use async_trait::async_trait;

pub struct BollardEngine;

impl BollardEngine {
    pub fn connect() -> Result<Self> {
        Ok(Self)
    }
}

#[async_trait]
impl DockerEngine for BollardEngine {
    async fn build_image(&self, _spec: &BuildImageSpec) -> Result<()> {
        unimplemented!("filled in Task 12")
    }
    async fn ensure_network(&self, _name: &str) -> Result<()> {
        unimplemented!("filled in Task 13")
    }
    async fn create_container(&self, _spec: &CreateContainerSpec) -> Result<String> {
        unimplemented!("filled in Task 13")
    }
    async fn start_container(&self, _id: &str) -> Result<()> {
        unimplemented!("filled in Task 13")
    }
    async fn container_exists(&self, _name: &str) -> Result<bool> {
        unimplemented!("filled in Task 13")
    }
}
