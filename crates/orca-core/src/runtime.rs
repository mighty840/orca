use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncRead;

use crate::error::Result;
use crate::types::{ResourceStats, WorkloadSpec, WorkloadStatus};

/// Opaque handle returned by a runtime after creating a workload.
#[derive(Debug, Clone)]
pub struct WorkloadHandle {
    /// Runtime-specific identifier (container ID, Wasm instance ID, etc.)
    pub runtime_id: String,
    /// Human-friendly name
    pub name: String,
}

/// Options for log retrieval.
#[derive(Debug, Clone, Default)]
pub struct LogOpts {
    pub follow: bool,
    pub tail: Option<u64>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub timestamps: bool,
}

/// Result of executing a command inside a workload.
#[derive(Debug)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// A stream of log bytes.
pub type LogStream = Pin<Box<dyn AsyncRead + Send>>;

/// The core runtime abstraction. Both container and Wasm runtimes implement this.
#[async_trait]
pub trait Runtime: Send + Sync + 'static {
    /// Human-readable name of this runtime (e.g., "container", "wasm").
    fn name(&self) -> &str;

    /// Create a new workload from the given spec. Does not start it.
    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle>;

    /// Start a previously created workload.
    async fn start(&self, handle: &WorkloadHandle) -> Result<()>;

    /// Stop a running workload, waiting up to `timeout` for graceful shutdown.
    async fn stop(&self, handle: &WorkloadHandle, timeout: Duration) -> Result<()>;

    /// Remove a stopped workload and clean up resources.
    async fn remove(&self, handle: &WorkloadHandle) -> Result<()>;

    /// Get the current status of a workload.
    async fn status(&self, handle: &WorkloadHandle) -> Result<WorkloadStatus>;

    /// Stream logs from a workload.
    async fn logs(&self, handle: &WorkloadHandle, opts: &LogOpts) -> Result<LogStream>;

    /// Execute a command inside a running workload.
    async fn exec(&self, handle: &WorkloadHandle, cmd: &[String]) -> Result<ExecResult>;

    /// Get current resource usage stats.
    async fn stats(&self, handle: &WorkloadHandle) -> Result<ResourceStats>;
}
