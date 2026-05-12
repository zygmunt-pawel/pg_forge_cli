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
        // bollard's connect_with_local_defaults reads DOCKER_HOST, then falls
        // back to /var/run/docker.sock. On macOS with Colima the socket lives
        // at ~/.colima/default/docker.sock, and `pgforge` invoked over SSH
        // (e.g. `ssh -t host pgforge`) doesn't load .zprofile so DOCKER_HOST
        // is unset. Probe Colima's well-known socket as a third fallback so
        // headless Mac setups work out of the box.
        if std::env::var_os("DOCKER_HOST").is_none()
            && !std::path::Path::new("/var/run/docker.sock").exists()
        {
            if let Some(home) = std::env::var_os("HOME") {
                let colima =
                    std::path::PathBuf::from(home).join(".colima/default/docker.sock");
                if colima.exists() {
                    // SAFETY: BollardEngine::connect is the single entry point used
                    // before any docker work; no other thread has read DOCKER_HOST yet.
                    unsafe {
                        std::env::set_var("DOCKER_HOST", format!("unix://{}", colima.display()));
                    }
                }
            }
        }
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

        let mut cfg = ContainerCreateBody {
            image: Some(spec.image.clone()),
            env: Some(env),
            exposed_ports: Some(exposed_ports),
            host_config: Some(host_config),
            ..Default::default()
        };

        if let Some(ep) = &spec.entrypoint_override {
            cfg.entrypoint = Some(ep.clone());
            // When the entrypoint is fully replaced, don't carry the image's
            // default CMD through — our entrypoint script needs to be the
            // single thing that runs (it chains to docker-entrypoint.sh itself
            // after its bootstrap steps).
            cfg.cmd = None;
        }
        if let Some(cmd) = &spec.cmd_override {
            cfg.cmd = Some(cmd.clone());
        }

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

    async fn container_running(&self, name: &str) -> Result<bool> {
        match self.docker.inspect_container(name, None).await {
            Ok(inspect) => Ok(inspect
                .state
                .as_ref()
                .and_then(|s| s.running)
                .unwrap_or(false)),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(false),
            Err(e) => Err(PgForgeError::Docker(format!(
                "inspect_container({name}): {e}"
            ))),
        }
    }

    async fn exec(&self, id: &str, cmd: &[&str]) -> Result<crate::docker::engine::ExecOutput> {
        use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
        use bollard::container::LogOutput;
        use crate::docker::engine::ExecOutput;

        let create = self
            .docker
            .create_exec(
                id,
                CreateExecOptions {
                    cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| PgForgeError::Docker(format!("create_exec: {e}")))?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        match self
            .docker
            .start_exec(&create.id, Some(StartExecOptions { detach: false, ..Default::default() }))
            .await
            .map_err(|e| PgForgeError::Docker(format!("start_exec: {e}")))?
        {
            StartExecResults::Attached { mut output, .. } => {
                while let Some(chunk) = output.next().await {
                    match chunk {
                        Ok(LogOutput::StdOut { message }) => {
                            stdout.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(LogOutput::StdErr { message }) => {
                            stderr.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(LogOutput::Console { message }) => {
                            stdout.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(_) => {}
                        Err(e) => return Err(PgForgeError::Docker(format!("exec stream: {e}"))),
                    }
                }
            }
            StartExecResults::Detached => {}
        }

        let inspect = self
            .docker
            .inspect_exec(&create.id)
            .await
            .map_err(|e| PgForgeError::Docker(format!("inspect_exec: {e}")))?;
        let exit_code = inspect.exit_code.unwrap_or(-1);
        Ok(ExecOutput { stdout, stderr, exit_code })
    }

    async fn stop_container(&self, id: &str) -> Result<()> {
        use bollard::query_parameters::StopContainerOptionsBuilder;
        let opts = StopContainerOptionsBuilder::default().t(10).build();
        self.docker
            .stop_container(id, Some(opts))
            .await
            .map_err(|e| PgForgeError::Docker(format!("stop_container({id}): {e}")))
    }

    async fn wait_for_container_running(
        &self,
        id: &str,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        loop {
            let inspect = self
                .docker
                .inspect_container(id, None)
                .await
                .map_err(|e| PgForgeError::Docker(format!("inspect_container: {e}")))?;
            let running = inspect
                .state
                .as_ref()
                .and_then(|s| s.running)
                .unwrap_or(false);
            if running {
                return Ok(());
            }
            if start.elapsed() >= timeout {
                return Err(PgForgeError::Docker(format!(
                    "container {id} did not reach Running state within {timeout:?}"
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    async fn wait_for_container_exit(
        &self,
        id: &str,
        timeout: std::time::Duration,
    ) -> Result<i64> {
        let start = std::time::Instant::now();
        loop {
            let inspect = self
                .docker
                .inspect_container(id, None)
                .await
                .map_err(|e| PgForgeError::Docker(format!("inspect_container: {e}")))?;
            let state = inspect.state.as_ref();
            let running = state.and_then(|s| s.running).unwrap_or(false);
            if !running {
                let exit_code = state.and_then(|s| s.exit_code).unwrap_or(-1);
                return Ok(exit_code);
            }
            if start.elapsed() >= timeout {
                return Err(PgForgeError::Docker(format!(
                    "container {id} did not exit within {timeout:?}"
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    async fn remove_container(&self, id: &str, force: bool) -> Result<()> {
        use bollard::query_parameters::RemoveContainerOptionsBuilder;
        let opts = RemoveContainerOptionsBuilder::default()
            .force(force)
            .v(true) // also remove anonymous volumes attached to this container
            .build();
        self.docker
            .remove_container(id, Some(opts))
            .await
            .map_err(|e| PgForgeError::Docker(format!("remove_container({id}): {e}")))
    }

    async fn remove_volume(&self, name: &str) -> Result<()> {
        use bollard::query_parameters::RemoveVolumeOptionsBuilder;
        let opts = RemoveVolumeOptionsBuilder::default().force(true).build();
        match self.docker.remove_volume(name, Some(opts)).await {
            Ok(_) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(()), // already gone — idempotent
            Err(e) => Err(PgForgeError::Docker(format!(
                "remove_volume({name}): {e}"
            ))),
        }
    }
}
