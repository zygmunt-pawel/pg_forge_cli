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

    async fn ensure_network(&self, name: &str) -> Result<()> {
        use bollard::query_parameters::ListNetworksOptionsBuilder;
        use std::collections::HashMap;

        let mut filters: HashMap<&str, Vec<&str>> = HashMap::new();
        filters.insert("name", vec![name]);

        let opts = ListNetworksOptionsBuilder::default()
            .filters(&filters)
            .build();

        let nets = self
            .docker
            .list_networks(Some(opts))
            .await
            .map_err(|e| PgForgeError::Docker(format!("list_networks: {e}")))?;

        if nets.iter().any(|n| n.name.as_deref() == Some(name)) {
            return Ok(());
        }

        let req = bollard::models::NetworkCreateRequest {
            name: name.to_string(),
            driver: Some("bridge".to_string()),
            ..Default::default()
        };

        self.docker
            .create_network(req)
            .await
            .map_err(|e| PgForgeError::Docker(format!("create_network({name}): {e}")))?;

        Ok(())
    }

    async fn create_container(&self, spec: &CreateContainerSpec) -> Result<String> {
        use bollard::models::{
            ContainerCreateBody, HostConfig, Mount, MountType, PortBinding, RestartPolicy,
            RestartPolicyNameEnum,
        };
        use bollard::query_parameters::CreateContainerOptionsBuilder;
        use std::collections::HashMap;

        // Port bindings: container_port/tcp -> host 127.0.0.1:host_port
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        port_bindings.insert(
            format!("{}/tcp", spec.container_port),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(spec.host_port.to_string()),
            }]),
        );

        // Exposed ports: list of "port/proto" strings
        let exposed_ports: Vec<String> = vec![format!("{}/tcp", spec.container_port)];

        // Mounts: bind mounts
        let mut mounts: Vec<Mount> = Vec::new();
        for b in &spec.binds {
            mounts.push(Mount {
                target: Some(b.container_path.clone()),
                source: Some(b.host_path.to_string_lossy().to_string()),
                typ: Some(MountType::BIND),
                read_only: Some(b.read_only),
                ..Default::default()
            });
        }
        // Mounts: named volumes
        for v in &spec.volumes {
            mounts.push(Mount {
                target: Some(v.container_path.clone()),
                source: Some(v.volume_name.clone()),
                typ: Some(MountType::VOLUME),
                ..Default::default()
            });
        }

        // Env vars as KEY=VALUE strings
        let env: Vec<String> = spec
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        let host_config = HostConfig {
            port_bindings: Some(port_bindings),
            mounts: Some(mounts),
            memory: Some((spec.memory_mb as i64) * 1024 * 1024),
            memory_swap: Some((spec.memory_mb as i64) * 1024 * 1024),
            shm_size: Some((spec.shm_size_mb as i64) * 1024 * 1024),
            network_mode: Some(spec.network.clone()),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                ..Default::default()
            }),
            ..Default::default()
        };

        let cfg = ContainerCreateBody {
            image: Some(spec.image.clone()),
            env: Some(env),
            exposed_ports: Some(exposed_ports),
            host_config: Some(host_config),
            ..Default::default()
        };

        let opts = CreateContainerOptionsBuilder::default()
            .name(spec.container_name.as_str())
            .build();

        let res = self
            .docker
            .create_container(Some(opts), cfg)
            .await
            .map_err(|e| PgForgeError::Docker(format!("create_container: {e}")))?;

        Ok(res.id)
    }

    async fn start_container(&self, id: &str) -> Result<()> {
        self.docker
            .start_container(id, None)
            .await
            .map_err(|e| PgForgeError::Docker(format!("start_container({id}): {e}")))
    }

    async fn container_exists(&self, name: &str) -> Result<bool> {
        use bollard::query_parameters::ListContainersOptionsBuilder;
        use std::collections::HashMap;

        // Docker prefixes container names with '/'; anchor the filter with regex.
        let mut filters: HashMap<&str, Vec<&str>> = HashMap::new();
        let pattern = format!("^/{name}$");
        filters.insert("name", vec![pattern.as_str()]);

        let opts = ListContainersOptionsBuilder::default()
            .all(true)
            .filters(&filters)
            .build();

        let list = self
            .docker
            .list_containers(Some(opts))
            .await
            .map_err(|e| PgForgeError::Docker(format!("list_containers: {e}")))?;

        Ok(!list.is_empty())
    }
}
