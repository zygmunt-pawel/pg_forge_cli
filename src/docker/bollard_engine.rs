use crate::docker::engine::{BuildImageSpec, CreateContainerSpec, DockerEngine};
use crate::error::{PgForgeError, Result};
use async_trait::async_trait;
use bollard::body_full;
use bollard::query_parameters::BuildImageOptionsBuilder;
use bollard::Docker;
use futures_util::StreamExt;
use std::io::Cursor;

pub struct BollardEngine {
    docker: Docker,
}

impl BollardEngine {
    pub fn connect() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| PgForgeError::Docker(format!("connect: {e}")))?;
        Ok(Self { docker })
    }

    /// Wrap a single Dockerfile into a TAR build context as bollard expects.
    fn make_tar_context(dockerfile: &str) -> Result<Vec<u8>> {
        let buf = Cursor::new(Vec::new());
        let mut builder = tar::Builder::new(buf);
        let bytes = dockerfile.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_path("Dockerfile").map_err(|e| {
            PgForgeError::Docker(format!("tar set_path: {e}"))
        })?;
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, bytes).map_err(|e| {
            PgForgeError::Docker(format!("tar append: {e}"))
        })?;
        let cursor = builder.into_inner().map_err(|e| {
            PgForgeError::Docker(format!("tar finish: {e}"))
        })?;
        Ok(cursor.into_inner())
    }
}

#[async_trait]
impl DockerEngine for BollardEngine {
    async fn build_image(&self, spec: &BuildImageSpec) -> Result<()> {
        let tar_bytes = Self::make_tar_context(&spec.dockerfile)?;

        let opts = BuildImageOptionsBuilder::default()
            .t(spec.tag.as_str())
            .dockerfile("Dockerfile")
            .rm(true)
            .forcerm(true)
            .build();

        let mut stream =
            self.docker
                .build_image(opts, None, Some(body_full(tar_bytes.into())));

        while let Some(item) = stream.next().await {
            match item {
                Ok(info) => {
                    if let Some(ref output) = info.stream {
                        let trimmed = output.trim();
                        if !trimmed.is_empty() {
                            tracing::debug!(target: "pgforge::docker::build", "{trimmed}");
                        }
                    }
                }
                Err(e) => return Err(PgForgeError::Docker(format!("build_image stream: {e}"))),
            }
        }
        Ok(())
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
