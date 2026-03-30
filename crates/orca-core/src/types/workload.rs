use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::gpu::GpuSpec;
use super::trigger::Trigger;
use super::{GpuStats, NodeId, RuntimeKind, WorkloadId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSpec {
    pub name: String,
    pub runtime: RuntimeKind,
    /// Container image (runtime = container) or Wasm module path/OCI ref (runtime = wasm)
    pub image: String,
    pub replicas: Replicas,
    pub port: Option<u16>,
    pub domain: Option<String>,
    pub health: Option<String>,
    pub env: std::collections::HashMap<String, String>,
    pub resources: Option<ResourceLimits>,
    pub volume: Option<VolumeSpec>,
    pub deploy: Option<DeployStrategy>,
    pub placement: Option<PlacementConstraint>,
    pub triggers: Vec<Trigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Replicas {
    Fixed(u32),
    Auto,
}

impl Default for Replicas {
    fn default() -> Self {
        Self::Fixed(1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub memory: Option<String>,
    pub cpu: Option<f64>,
    /// GPU requirements. If set, scheduler places workload on GPU-equipped nodes.
    pub gpu: Option<GpuSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub path: String,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStrategy {
    #[serde(default = "default_strategy")]
    pub strategy: DeployKind,
    pub max_unavailable: Option<u32>,
}

fn default_strategy() -> DeployKind {
    DeployKind::Rolling
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeployKind {
    Rolling,
    BlueGreen,
    Canary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementConstraint {
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub node: Option<String>,
    /// Require GPU-equipped node.
    pub requires_gpu: Option<bool>,
}

// -- Workload Instance --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadInstance {
    pub id: WorkloadId,
    pub spec_name: String,
    pub node_id: NodeId,
    pub runtime: RuntimeKind,
    pub status: WorkloadStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub health: HealthState,
    /// GPU device indices assigned to this workload.
    pub gpu_devices: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkloadStatus {
    Pending,
    Creating,
    Running,
    Stopping,
    Stopped,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    Unknown,
    Healthy,
    Unhealthy,
    NoCheck,
}

// -- Resource Stats --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceStats {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub gpu_stats: Vec<GpuStats>,
    pub timestamp: DateTime<Utc>,
}
