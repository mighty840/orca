//! Docker container runtime implementing the [`Runtime`] trait via bollard.

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, InspectContainerOptions, ListContainersOptions, LogsOptions,
    RemoveContainerOptions, StartContainerOptions, Stats, StatsOptions, StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, PortBinding};
use chrono::Utc;
use futures_util::StreamExt;
use tokio::io::AsyncRead;
use tokio_util::io::StreamReader;
use tracing::{debug, info, warn};

use orca_core::error::{OrcaError, Result};
use orca_core::runtime::{ExecResult, LogOpts, LogStream, Runtime, WorkloadHandle};
use orca_core::types::{ResourceStats, WorkloadSpec, WorkloadStatus};

/// Label applied to all orca-managed containers for identification and cleanup.
const ORCA_LABEL: &str = "orca.managed";

/// Docker container runtime.
pub struct ContainerRuntime {
    docker: Docker,
}

impl ContainerRuntime {
    /// Connect to the local Docker daemon.
    ///
    /// # Errors
    ///
    /// Returns an error if the Docker socket is not available.
    pub fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| OrcaError::Runtime(format!("failed to connect to Docker: {e}")))?;
        Ok(Self { docker })
    }

    /// Pull an image if it does not exist locally.
    async fn ensure_image(&self, image: &str) -> Result<()> {
        // Check if image exists locally
        if self.docker.inspect_image(image).await.is_ok() {
            debug!("Image {image} already available locally");
            return Ok(());
        }

        info!("Pulling image: {image}");
        let opts = CreateImageOptions {
            from_image: image,
            ..Default::default()
        };

        let mut stream = self.docker.create_image(Some(opts), None, None);
        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(status) = &info.status {
                        debug!("Pull {image}: {status}");
                    }
                }
                Err(e) => {
                    return Err(OrcaError::Runtime(format!(
                        "failed to pull image {image}: {e}"
                    )));
                }
            }
        }

        info!("Image pulled: {image}");
        Ok(())
    }

    /// Inspect a container and return its assigned host port for the primary port.
    pub async fn get_host_port(
        &self,
        container_id: &str,
        container_port: u16,
    ) -> Result<Option<u16>> {
        let info = self
            .docker
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
            .map_err(|e| OrcaError::Runtime(format!("inspect failed: {e}")))?;

        let port_key = format!("{container_port}/tcp");
        let host_port = info
            .network_settings
            .and_then(|ns| ns.ports)
            .and_then(|ports| ports.get(&port_key).cloned())
            .and_then(|bindings| bindings)
            .and_then(|bindings| bindings.into_iter().next())
            .and_then(|b| b.host_port)
            .and_then(|p| p.parse::<u16>().ok());

        Ok(host_port)
    }

    /// List all orca-managed containers.
    pub async fn list_managed_containers(&self) -> Result<Vec<String>> {
        let mut filters = HashMap::new();
        filters.insert("label", vec![ORCA_LABEL]);
        let opts = ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        };
        let containers = self
            .docker
            .list_containers(Some(opts))
            .await
            .map_err(|e| OrcaError::Runtime(format!("list containers failed: {e}")))?;

        Ok(containers.into_iter().filter_map(|c| c.id).collect())
    }

    /// Stop and remove all orca-managed containers. Used for graceful shutdown.
    pub async fn cleanup_all(&self) {
        match self.list_managed_containers().await {
            Ok(ids) => {
                for id in ids {
                    let short = &id[..12.min(id.len())];
                    debug!("Cleaning up container {short}");
                    let _ = self
                        .docker
                        .stop_container(&id, Some(StopContainerOptions { t: 5 }))
                        .await;
                    let _ = self
                        .docker
                        .remove_container(
                            &id,
                            Some(RemoveContainerOptions {
                                force: true,
                                ..Default::default()
                            }),
                        )
                        .await;
                }
            }
            Err(e) => {
                warn!("Failed to list containers for cleanup: {e}");
            }
        }
    }
}

#[async_trait]
impl Runtime for ContainerRuntime {
    fn name(&self) -> &str {
        "container"
    }

    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle> {
        self.ensure_image(&spec.image).await?;

        let container_name = format!("orca-{}", spec.name);

        // Build environment variables
        let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

        // Build port bindings: publish the container port to an ephemeral host port
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
        if let Some(port) = spec.port {
            let key = format!("{port}/tcp");
            exposed_ports.insert(key.clone(), HashMap::new());
            port_bindings.insert(
                key,
                Some(vec![PortBinding {
                    host_ip: Some("0.0.0.0".to_string()),
                    host_port: Some("0".to_string()), // let Docker pick
                }]),
            );
        }

        // Build volume binds
        let mut binds = Vec::new();
        if let Some(vol) = &spec.volume {
            // Named volume: orca-{service}-data:{path}
            let vol_name = format!("orca-{}-data", spec.name);
            binds.push(format!("{vol_name}:{}", vol.path));
        }

        // GPU device requests (nvidia)
        let mut device_requests = Vec::new();
        if let Some(res) = &spec.resources
            && let Some(gpu) = &res.gpu
        {
            device_requests.push(bollard::models::DeviceRequest {
                count: Some(gpu.count as i64),
                driver: Some("nvidia".to_string()),
                capabilities: Some(vec![vec!["gpu".to_string()]]),
                ..Default::default()
            });
        }

        // Labels
        let mut labels = HashMap::new();
        labels.insert(ORCA_LABEL.to_string(), "true".to_string());
        labels.insert("orca.service".to_string(), spec.name.clone());

        let host_config = HostConfig {
            port_bindings: Some(port_bindings),
            binds: if binds.is_empty() { None } else { Some(binds) },
            device_requests: if device_requests.is_empty() {
                None
            } else {
                Some(device_requests)
            },
            ..Default::default()
        };

        let config = Config {
            image: Some(spec.image.clone()),
            env: if env.is_empty() { None } else { Some(env) },
            exposed_ports: if exposed_ports.is_empty() {
                None
            } else {
                Some(exposed_ports)
            },
            host_config: Some(host_config),
            labels: Some(labels),
            ..Default::default()
        };

        let opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };

        // Remove existing container with same name if it exists
        let _ = self
            .docker
            .remove_container(
                &container_name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        let response = self
            .docker
            .create_container(Some(opts), config)
            .await
            .map_err(|e| OrcaError::Runtime(format!("create container failed: {e}")))?;

        info!(
            "Created container {} ({})",
            container_name,
            &response.id[..12]
        );

        Ok(WorkloadHandle {
            runtime_id: response.id,
            name: container_name,
            metadata: HashMap::new(),
        })
    }

    async fn start(&self, handle: &WorkloadHandle) -> Result<()> {
        self.docker
            .start_container(&handle.runtime_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| OrcaError::Runtime(format!("start container failed: {e}")))?;

        info!("Started container {}", handle.name);
        Ok(())
    }

    async fn stop(&self, handle: &WorkloadHandle, timeout: Duration) -> Result<()> {
        let t = timeout.as_secs() as i64;
        self.docker
            .stop_container(&handle.runtime_id, Some(StopContainerOptions { t }))
            .await
            .map_err(|e| OrcaError::Runtime(format!("stop container failed: {e}")))?;

        info!("Stopped container {}", handle.name);
        Ok(())
    }

    async fn remove(&self, handle: &WorkloadHandle) -> Result<()> {
        self.docker
            .remove_container(
                &handle.runtime_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(|e| OrcaError::Runtime(format!("remove container failed: {e}")))?;

        info!("Removed container {}", handle.name);
        Ok(())
    }

    async fn status(&self, handle: &WorkloadHandle) -> Result<WorkloadStatus> {
        let info = self
            .docker
            .inspect_container(&handle.runtime_id, None::<InspectContainerOptions>)
            .await
            .map_err(|e| OrcaError::Runtime(format!("inspect failed: {e}")))?;

        let state = info.state.as_ref();
        let status_str = state
            .and_then(|s| s.status.as_ref())
            .map(|s| format!("{s:?}"))
            .unwrap_or_default()
            .to_lowercase();

        let ws = match status_str.as_str() {
            "running" => WorkloadStatus::Running,
            "created" => WorkloadStatus::Creating,
            "exited" | "dead" => {
                let exit_code = state.and_then(|s| s.exit_code).unwrap_or(-1);
                if exit_code == 0 {
                    WorkloadStatus::Completed
                } else {
                    WorkloadStatus::Failed
                }
            }
            "paused" | "restarting" => WorkloadStatus::Running,
            _ => WorkloadStatus::Stopped,
        };

        Ok(ws)
    }

    async fn logs(&self, handle: &WorkloadHandle, opts: &LogOpts) -> Result<LogStream> {
        let log_opts = LogsOptions::<String> {
            follow: opts.follow,
            stdout: true,
            stderr: true,
            tail: opts
                .tail
                .map(|t| t.to_string())
                .unwrap_or_else(|| "100".to_string()),
            timestamps: opts.timestamps,
            ..Default::default()
        };

        let stream = self.docker.logs(&handle.runtime_id, Some(log_opts));

        // Convert the bollard log stream into an AsyncRead
        let byte_stream = stream.map(|result| match result {
            Ok(output) => Ok(output.into_bytes()),
            Err(e) => Err(std::io::Error::other(e.to_string())),
        });

        let reader = StreamReader::new(byte_stream);
        Ok(Box::pin(reader) as Pin<Box<dyn AsyncRead + Send>>)
    }

    async fn exec(&self, handle: &WorkloadHandle, cmd: &[String]) -> Result<ExecResult> {
        let exec = self
            .docker
            .create_exec(
                &handle.runtime_id,
                CreateExecOptions {
                    cmd: Some(cmd.to_vec()),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| OrcaError::Runtime(format!("create exec failed: {e}")))?;

        let output = self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|e| OrcaError::Runtime(format!("start exec failed: {e}")))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        if let StartExecResults::Attached { mut output, .. } = output {
            while let Some(Ok(msg)) = output.next().await {
                match msg {
                    bollard::container::LogOutput::StdOut { message } => {
                        stdout.extend_from_slice(&message);
                    }
                    bollard::container::LogOutput::StdErr { message } => {
                        stderr.extend_from_slice(&message);
                    }
                    _ => {}
                }
            }
        }

        // Get exit code
        let inspect = self
            .docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| OrcaError::Runtime(format!("inspect exec failed: {e}")))?;
        let exit_code = inspect.exit_code.unwrap_or(-1) as i32;

        Ok(ExecResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    async fn stats(&self, handle: &WorkloadHandle) -> Result<ResourceStats> {
        let opts = StatsOptions {
            stream: false,
            one_shot: true,
        };

        let mut stream = self.docker.stats(&handle.runtime_id, Some(opts));

        if let Some(Ok(stats)) = stream.next().await {
            let cpu_percent = calculate_cpu_percent(&stats);
            let memory_bytes = stats.memory_stats.usage.unwrap_or(0);
            let (rx, tx) = extract_network_stats(&stats);

            Ok(ResourceStats {
                cpu_percent,
                memory_bytes,
                network_rx_bytes: rx,
                network_tx_bytes: tx,
                gpu_stats: Vec::new(), // GPU stats require nvidia-smi, not available via Docker API
                timestamp: Utc::now(),
            })
        } else {
            Err(OrcaError::Runtime("no stats available".to_string()))
        }
    }

    async fn resolve_host_port(
        &self,
        handle: &WorkloadHandle,
        container_port: u16,
    ) -> Result<Option<u16>> {
        self.get_host_port(&handle.runtime_id, container_port).await
    }
}

/// Calculate CPU usage percentage from Docker stats.
fn calculate_cpu_percent(stats: &Stats) -> f64 {
    let cpu_delta = stats.cpu_stats.cpu_usage.total_usage as f64
        - stats.precpu_stats.cpu_usage.total_usage as f64;
    let system_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0) as f64
        - stats.precpu_stats.system_cpu_usage.unwrap_or(0) as f64;
    let num_cpus = stats.cpu_stats.online_cpus.unwrap_or(1) as f64;

    if system_delta > 0.0 && cpu_delta >= 0.0 {
        (cpu_delta / system_delta) * num_cpus * 100.0
    } else {
        0.0
    }
}

/// Extract network RX/TX bytes from Docker stats.
fn extract_network_stats(stats: &Stats) -> (u64, u64) {
    stats
        .networks
        .as_ref()
        .map(|networks| {
            networks.values().fold((0u64, 0u64), |(rx, tx), iface| {
                (rx + iface.rx_bytes, tx + iface.tx_bytes)
            })
        })
        .unwrap_or((0, 0))
}
