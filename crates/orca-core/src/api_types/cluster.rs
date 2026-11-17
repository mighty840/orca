//! `GET /api/v1/cluster/info`, typed so the TUI, the web console (#259) and
//! MCP (#80) parse one shape instead of each picking fields out of JSON.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::NodeGpuConfig;

/// Response from `GET /api/v1/cluster/info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfoResponse {
    pub cluster_name: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub acme_email: Option<String>,
    /// Version of the master's binary.
    pub version: String,
    /// Git commit of the master's binary, or "unknown".
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub nodes: Vec<ClusterNode>,
    #[serde(default)]
    pub node_count: usize,
}

/// One registered node: everything the master knows about it, plus the
/// GPUs `cluster.toml` declares for it. Missing fields default, so an older
/// master's response still parses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    pub node_id: u64,
    pub address: String,
    /// The control session's peer IP as the master sees it.
    #[serde(default)]
    pub peer_ip: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub drain: bool,
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub memory_bytes: u64,
    #[serde(default)]
    pub memory_total: u64,
    #[serde(default)]
    pub disk_used: u64,
    #[serde(default)]
    pub disk_total: u64,
    #[serde(default)]
    pub net_rx: u64,
    #[serde(default)]
    pub net_tx: u64,
    #[serde(default)]
    pub gpus: Vec<NodeGpuConfig>,
}
