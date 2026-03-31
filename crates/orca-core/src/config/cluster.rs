use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::ai::AiConfig;
use crate::backup::BackupConfig;

/// Top-level cluster configuration (`cluster.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterConfig {
    pub cluster: ClusterMeta,
    #[serde(default)]
    pub node: Vec<NodeConfig>,
    #[serde(default)]
    pub observability: Option<ObservabilityConfig>,
    #[serde(default)]
    pub ai: Option<AiConfig>,
    #[serde(default)]
    pub backup: Option<BackupConfig>,
    /// API bearer tokens for authentication. Empty = allow all requests.
    #[serde(default)]
    pub api_tokens: Vec<String>,
    /// Mesh networking configuration (NetBird).
    #[serde(default)]
    pub network: Option<NetworkConfig>,
}

/// Mesh networking configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Network provider: "netbird" (default).
    #[serde(default = "default_network_provider")]
    pub provider: String,
    /// NetBird setup key for joining the mesh.
    pub setup_key: Option<String>,
    /// NetBird management URL (default: api.netbird.io).
    pub management_url: Option<String>,
}

fn default_network_provider() -> String {
    "netbird".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMeta {
    #[serde(default = "default_cluster_name")]
    pub name: String,
    pub domain: Option<String>,
    pub acme_email: Option<String>,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_api_port")]
    pub api_port: u16,
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
}

impl Default for ClusterMeta {
    fn default() -> Self {
        Self {
            name: default_cluster_name(),
            domain: None,
            acme_email: None,
            log_level: default_log_level(),
            api_port: default_api_port(),
            grpc_port: default_grpc_port(),
        }
    }
}

fn default_cluster_name() -> String {
    "orca".into()
}

pub(crate) fn default_log_level() -> String {
    "info".into()
}

pub(crate) fn default_api_port() -> u16 {
    6880
}

pub(crate) fn default_grpc_port() -> u16 {
    6881
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub address: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// GPU devices available on this node.
    #[serde(default)]
    pub gpus: Vec<NodeGpuConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeGpuConfig {
    /// Vendor: "nvidia" or "amd".
    pub vendor: String,
    /// Number of GPUs of this type.
    #[serde(default = "default_gpu_count")]
    pub count: u32,
    /// Model name for scheduling (e.g., "A100", "RTX4090").
    pub model: Option<String>,
}

pub(crate) fn default_gpu_count() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub otlp_endpoint: Option<String>,
    pub alerts: Option<AlertChannelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertChannelConfig {
    pub webhook: Option<String>,
    pub email: Option<String>,
}
