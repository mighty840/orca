//! Mock implementation of the [`Runtime`] trait for testing.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;

use crate::error::{OrcaError, Result};
use crate::runtime::{AsAny, ExecResult, LogOpts, LogStream, Runtime, WorkloadHandle};
use crate::types::{ResourceStats, WorkloadSpec, WorkloadStatus};

/// Records of operations performed on the mock runtime.
#[derive(Debug, Clone)]
pub enum MockOp {
    /// A workload was created.
    Create(String),
    /// A workload was started.
    Start(String),
    /// A workload was stopped.
    Stop(String),
    /// A workload was removed.
    Remove(String),
}

/// A mock [`Runtime`] that tracks operations without running real workloads.
///
/// Use this in integration tests to verify reconciler behavior,
/// API endpoints, and other components that depend on a runtime.
pub struct MockRuntime {
    /// Recorded operations, in order.
    pub ops: Arc<Mutex<Vec<MockOp>>>,
    /// Current status per runtime_id.
    statuses: Arc<Mutex<HashMap<String, WorkloadStatus>>>,
    /// Counter for generating unique IDs.
    counter: Arc<Mutex<u64>>,
    /// If set, the mock host port returned by resolve_host_port.
    pub mock_host_port: Option<u16>,
}

impl MockRuntime {
    /// Create a new mock runtime.
    pub fn new() -> Self {
        Self {
            ops: Arc::new(Mutex::new(Vec::new())),
            statuses: Arc::new(Mutex::new(HashMap::new())),
            counter: Arc::new(Mutex::new(0)),
            mock_host_port: None,
        }
    }

    /// Create a mock runtime that returns a fixed host port.
    pub fn with_host_port(port: u16) -> Self {
        Self {
            mock_host_port: Some(port),
            ..Self::new()
        }
    }

    /// Get a copy of all recorded operations.
    pub async fn recorded_ops(&self) -> Vec<MockOp> {
        self.ops.lock().await.clone()
    }
}

impl Default for MockRuntime {
    fn default() -> Self {
        Self::new()
    }
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
        let mut counter = self.counter.lock().await;
        *counter += 1;
        let id = format!("mock-{}", *counter);

        self.ops
            .lock()
            .await
            .push(MockOp::Create(spec.name.clone()));
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

    async fn start(&self, handle: &WorkloadHandle) -> Result<()> {
        self.ops
            .lock()
            .await
            .push(MockOp::Start(handle.name.clone()));
        self.statuses
            .lock()
            .await
            .insert(handle.runtime_id.clone(), WorkloadStatus::Running);
        Ok(())
    }

    async fn stop(&self, handle: &WorkloadHandle, _timeout: Duration) -> Result<()> {
        self.ops
            .lock()
            .await
            .push(MockOp::Stop(handle.name.clone()));
        self.statuses
            .lock()
            .await
            .insert(handle.runtime_id.clone(), WorkloadStatus::Stopped);
        Ok(())
    }

    async fn remove(&self, handle: &WorkloadHandle) -> Result<()> {
        self.ops
            .lock()
            .await
            .push(MockOp::Remove(handle.name.clone()));
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
