use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bollard::container::{
    CreateContainerOptions, InspectContainerOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use futures_util::StreamExt;
use tokio::io::AsyncRead;
use tokio_util::io::StreamReader;
use tracing::info;

use orca_core::error::{OrcaError, Result};
use orca_core::runtime::{ExecResult, LogOpts, LogStream, Runtime, WorkloadHandle};
use orca_core::types::{ResourceStats, WorkloadSpec, WorkloadStatus};

use super::ContainerRuntime;

#[async_trait]
impl Runtime for ContainerRuntime {
    fn name(&self) -> &str {
        "container"
    }

    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle> {
        self.ensure_image(&spec.image).await?;

        let container_name = format!("orca-{}", spec.name);
        let config = super::config_builder::build_container_config(spec);
        let network = super::config_builder::network_name(spec);

        // Ensure the service network exists
        let _ = self.ensure_network(&network).await;

        let opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };

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

        // Connect to service network with aliases
        let mut aliases = spec.aliases.clone();
        aliases.push(container_name.clone());
        let _ = self
            .connect_to_network(&response.id, &network, &aliases)
            .await;

        // Also connect to orca-internal network for cross-service communication
        if spec.internal {
            let _ = self.ensure_network("orca-internal").await;
            let _ = self
                .connect_to_network(
                    &response.id,
                    "orca-internal",
                    std::slice::from_ref(&container_name),
                )
                .await;
        }

        info!(
            "Created container {} ({}) on {network}{}",
            container_name,
            &response.id[..12],
            if spec.internal {
                " + orca-internal"
            } else {
                ""
            }
        );

        Ok(WorkloadHandle {
            runtime_id: response.id,
            name: container_name,
            metadata: std::collections::HashMap::new(),
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
        super::stats::collect_stats(&self.docker, &handle.runtime_id).await
    }

    async fn resolve_host_port(
        &self,
        handle: &WorkloadHandle,
        container_port: u16,
    ) -> Result<Option<u16>> {
        self.get_host_port(&handle.runtime_id, container_port).await
    }

    async fn resolve_container_address(
        &self,
        handle: &WorkloadHandle,
        container_port: u16,
        network: &str,
    ) -> Result<Option<String>> {
        if let Some(ip) = self.get_container_ip(&handle.runtime_id, network).await? {
            Ok(Some(format!("{ip}:{container_port}")))
        } else {
            Ok(None)
        }
    }
}
