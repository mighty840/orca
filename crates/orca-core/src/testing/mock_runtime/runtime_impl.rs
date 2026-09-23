//! [`Runtime`] implementation for [`MockRuntime`].

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use super::{MockOp, MockOpKind, MockRuntime};
use crate::error::{OrcaError, Result};
use crate::runtime::{AsAny, ExecResult, LogOpts, LogStream, Runtime, WorkloadHandle};
use crate::types::{ResourceStats, WorkloadSpec, WorkloadStatus};

/// Build the error returned for an injected failure.
fn injected(kind: MockOpKind, target: &str) -> OrcaError {
    OrcaError::Runtime(format!(
        "mock: injected {} failure for {target}",
        kind.as_str()
    ))
}

impl AsAny for MockRuntime {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait]
impl Runtime for MockRuntime {
    fn name(&self) -> &str {
        "mock"
    }

    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle> {
        if self.take_failure(MockOpKind::Create).await {
            return Err(injected(MockOpKind::Create, &spec.name));
        }

        let id = {
            let mut counter = self.counter.lock().await;
            *counter += 1;
            format!("mock-{}", *counter)
        };

        self.record(MockOp::Create(spec.name.clone())).await;
        self.fingerprints
            .lock()
            .await
            .insert(format!("orca-{}", spec.name), spec.fingerprint.clone());
        self.statuses
            .lock()
            .await
            .insert(id.clone(), WorkloadStatus::Creating);

        Ok(WorkloadHandle {
            runtime_id: id,
            name: format!("orca-{}", spec.name),
            metadata: HashMap::new(),
        })
    }

    async fn spec_fingerprint(&self, handle: &WorkloadHandle) -> Result<Option<String>> {
        let fps = self.fingerprints.lock().await;
        Ok(fps
            .get(&handle.name)
            .or_else(|| fps.get(&handle.runtime_id))
            .cloned()
            .flatten())
    }

    async fn start(&self, handle: &WorkloadHandle) -> Result<()> {
        if self.take_failure(MockOpKind::Start).await {
            return Err(injected(MockOpKind::Start, &handle.name));
        }

        self.record(MockOp::Start(handle.name.clone())).await;
        self.statuses
            .lock()
            .await
            .insert(handle.runtime_id.clone(), WorkloadStatus::Running);
        Ok(())
    }

    async fn stop(&self, handle: &WorkloadHandle, timeout: Duration) -> Result<()> {
        if self.take_failure(MockOpKind::Stop).await {
            return Err(injected(MockOpKind::Stop, &handle.name));
        }

        // The grace period is recorded, not ignored: tests assert that a
        // recreate stops the old workload gracefully before removing it.
        self.record(MockOp::Stop {
            name: handle.name.clone(),
            timeout,
        })
        .await;
        self.statuses
            .lock()
            .await
            .insert(handle.runtime_id.clone(), WorkloadStatus::Stopped);
        Ok(())
    }

    async fn remove(&self, handle: &WorkloadHandle) -> Result<()> {
        if self.take_failure(MockOpKind::Remove).await {
            return Err(injected(MockOpKind::Remove, &handle.name));
        }

        self.record(MockOp::Remove(handle.name.clone())).await;
        self.statuses.lock().await.remove(&handle.runtime_id);
        Ok(())
    }

    async fn status(&self, handle: &WorkloadHandle) -> Result<WorkloadStatus> {
        let statuses = self.statuses.lock().await;
        statuses
            .get(&handle.runtime_id)
            .copied()
            .ok_or_else(|| OrcaError::WorkloadNotFound {
                name: handle.runtime_id.clone(),
            })
    }

    async fn logs(&self, _handle: &WorkloadHandle, _opts: &LogOpts) -> Result<LogStream> {
        let text = b"mock log line 1\nmock log line 2\n";
        let cursor = std::io::Cursor::new(text.to_vec());
        Ok(Box::pin(cursor) as Pin<Box<dyn tokio::io::AsyncRead + Send>>)
    }

    async fn exec(&self, _handle: &WorkloadHandle, cmd: &[String]) -> Result<ExecResult> {
        Ok(ExecResult {
            exit_code: 0,
            stdout: format!("mock exec: {}", cmd.join(" ")).into_bytes(),
            stderr: Vec::new(),
        })
    }

    async fn stats(&self, _handle: &WorkloadHandle) -> Result<ResourceStats> {
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
        Ok(self.mock_host_port)
    }
}
