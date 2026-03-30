//! [`Runtime`] trait implementation for [`WasmRuntime`].

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::io::AsyncRead;
use tracing::info;

use orca_core::error::{OrcaError, Result};
use orca_core::runtime::{ExecResult, LogOpts, LogStream, Runtime, WorkloadHandle};
use orca_core::types::{ResourceStats, WorkloadSpec, WorkloadStatus};

use super::runtime::WasmRuntime;
use super::state::WasmInstance;

#[async_trait]
impl Runtime for WasmRuntime {
    fn name(&self) -> &str {
        "wasm"
    }

    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle> {
        let component = self.load_component(&spec.image).await?;

        let runtime_id = format!("wasm-{}", uuid::Uuid::now_v7());
        info!("Created Wasm instance {} for {}", runtime_id, spec.name);

        let instance = WasmInstance {
            component,
            status: WorkloadStatus::Creating,
            spec: spec.clone(),
            logs: Vec::new(),
            started_at: None,
            invocation_count: 0,
        };

        let mut instances = self.instances.write().await;
        instances.insert(runtime_id.clone(), instance);

        Ok(WorkloadHandle {
            runtime_id,
            name: format!("orca-wasm-{}", spec.name),
            metadata: HashMap::new(),
        })
    }

    async fn start(&self, handle: &WorkloadHandle) -> Result<()> {
        let mut instances = self.instances.write().await;
        let instance =
            instances
                .get_mut(&handle.runtime_id)
                .ok_or_else(|| OrcaError::WorkloadNotFound {
                    name: handle.runtime_id.clone(),
                })?;

        // For Wasm, "starting" means the component is ready to receive invocations.
        // Unlike containers, there's no long-running process — each HTTP trigger
        // creates a fresh Store and calls the component's export.
        instance.status = WorkloadStatus::Running;
        instance.started_at = Some(Utc::now());
        instance.logs.push(format!(
            "{} Instance started (trigger-based, no background process)",
            Utc::now().format("%H:%M:%S")
        ));

        info!("Wasm instance {} is ready", handle.name);
        Ok(())
    }

    async fn stop(&self, handle: &WorkloadHandle, _timeout: Duration) -> Result<()> {
        let mut instances = self.instances.write().await;
        if let Some(instance) = instances.get_mut(&handle.runtime_id) {
            instance.status = WorkloadStatus::Stopped;
            instance.logs.push(format!(
                "{} Instance stopped (invocations: {})",
                Utc::now().format("%H:%M:%S"),
                instance.invocation_count
            ));
            info!("Stopped Wasm instance {}", handle.name);
        }
        Ok(())
    }

    async fn remove(&self, handle: &WorkloadHandle) -> Result<()> {
        let mut instances = self.instances.write().await;
        instances.remove(&handle.runtime_id);
        info!("Removed Wasm instance {}", handle.name);
        Ok(())
    }

    async fn status(&self, handle: &WorkloadHandle) -> Result<WorkloadStatus> {
        let instances = self.instances.read().await;
        let instance =
            instances
                .get(&handle.runtime_id)
                .ok_or_else(|| OrcaError::WorkloadNotFound {
                    name: handle.runtime_id.clone(),
                })?;
        Ok(instance.status)
    }

    async fn logs(&self, handle: &WorkloadHandle, opts: &LogOpts) -> Result<LogStream> {
        let instances = self.instances.read().await;
        let instance =
            instances
                .get(&handle.runtime_id)
                .ok_or_else(|| OrcaError::WorkloadNotFound {
                    name: handle.runtime_id.clone(),
                })?;

        let tail = opts.tail.unwrap_or(100) as usize;
        let logs: Vec<String> = instance
            .logs
            .iter()
            .rev()
            .take(tail)
            .rev()
            .cloned()
            .collect();

        let text = logs.join("\n") + "\n";
        let cursor = std::io::Cursor::new(text.into_bytes());
        Ok(Box::pin(cursor) as Pin<Box<dyn AsyncRead + Send>>)
    }

    async fn exec(&self, _handle: &WorkloadHandle, _cmd: &[String]) -> Result<ExecResult> {
        Err(OrcaError::Runtime(
            "exec is not supported for Wasm workloads".to_string(),
        ))
    }

    async fn stats(&self, handle: &WorkloadHandle) -> Result<ResourceStats> {
        let instances = self.instances.read().await;
        let _instance =
            instances
                .get(&handle.runtime_id)
                .ok_or_else(|| OrcaError::WorkloadNotFound {
                    name: handle.runtime_id.clone(),
                })?;

        // Wasm instances are ephemeral — stats are approximate.
        // Memory is per-invocation and freed after each call.
        Ok(ResourceStats {
            cpu_percent: 0.0,
            memory_bytes: 0,
            network_rx_bytes: 0,
            network_tx_bytes: 0,
            gpu_stats: Vec::new(),
            timestamp: Utc::now(),
        })
    }

    async fn resolve_host_port(
        &self,
        _handle: &WorkloadHandle,
        _container_port: u16,
    ) -> Result<Option<u16>> {
        // Wasm workloads don't bind ports — they're invoked via HTTP triggers
        // routed through the proxy.
        Ok(None)
    }
}
