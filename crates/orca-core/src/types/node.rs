use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::NodeId;
use super::gpu::GpuInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: NodeId,
    pub address: String,
    pub labels: std::collections::HashMap<String, String>,
    pub status: NodeStatus,
    pub resources: NodeResources,
    pub joined_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Ready,
    NotReady,
    Draining,
    Left,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResources {
    pub cpu_cores: f64,
    pub memory_bytes: u64,
    pub cpu_used: f64,
    pub memory_used: u64,
    /// GPUs available on this node.
    pub gpus: Vec<GpuInfo>,
}
