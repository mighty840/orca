use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bollard::container::{
    CreateContainerOptions, InspectContainerOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StatsOptions, StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use chrono::Utc;
use futures_util::StreamExt;
use tokio::io::AsyncRead;
use tokio_util::io::StreamReader;
use tracing::info;

use orca_core::error::{OrcaError, Result};
use orca_core::runtime::{ExecResult, LogOpts, LogStream, Runtime, WorkloadHandle};
use orca_core::types::{ResourceStats, WorkloadSpec, WorkloadStatus};

use super::ContainerRuntime;

use super::stats::{calculate_cpu_percent, extract_network_stats};

#[async_trait]
impl Runtime for ContainerRuntime {
    fn name(&self) -> &str {
        "container"
    }

    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle> {
        self.ensure_image(&spec.image).await?;

        let container_name = format!("orca-{}", spec.name);
        let config = super::config_builder::build_container_config(spec);

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
                gpu_stats: Vec::new(),
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
