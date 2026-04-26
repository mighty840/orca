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
    #[serde(default)]
    pub cleanup: Option<CleanupConfig>,
    /// API bearer tokens for authentication. Empty = allow all requests.
    /// Deprecated: use `[[token]]` entries with roles instead.
    #[serde(default)]
    pub api_tokens: Vec<String>,
    /// Named API tokens with role-based access control.
    #[serde(default)]
    pub token: Vec<ApiToken>,
    /// Mesh networking configuration (NetBird).
    #[serde(default)]
    pub network: Option<NetworkConfig>,
    /// Fallback proxy for unmatched requests (e.g., point to coolify-proxy).
    #[serde(default)]
    pub fallback: Option<FallbackConfig>,
}

/// Fallback proxy configuration. When orca's route table has no match,
/// requests are forwarded here. Lets orca coexist with another reverse proxy.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FallbackConfig {
    /// HTTP fallback target (e.g., "127.0.0.1:8081").
    pub http: Option<String>,
    /// HTTPS/TLS fallback target for SNI passthrough (e.g., "127.0.0.1:8443").
    pub tls: Option<String>,
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

/// Named API token with role-based access control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    /// Human-readable name (e.g., "sharang", "gitea-ci").
    pub name: String,
    /// Bearer token value.
    pub value: String,
    /// Role: admin, deployer, or viewer.
    #[serde(default = "default_role")]
    pub role: Role,
}

/// Access control role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Full access: deploy, stop, scale, drain, manage tokens.
    #[default]
    Admin,
    /// Deploy, stop, scale, logs, status. For CI/CD service accounts.
    Deployer,
    /// Read-only: status, logs, metrics. For dashboards.
    Viewer,
}

impl Role {
    /// Check if this role can perform the given action.
    ///
    /// `secrets` is admin-only. Everything else escalates from viewer →
    /// deployer → admin in the obvious way.
    pub fn can(self, action: &str) -> bool {
        match self {
            Role::Admin => true,
            Role::Deployer => matches!(
                action,
                "deploy" | "stop" | "scale" | "rollback" | "status" | "logs" | "cluster_info"
            ),
            Role::Viewer => matches!(action, "status" | "logs" | "cluster_info"),
        }
    }
}

fn default_role() -> Role {
    Role::Admin
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

/// Scheduled cleanup configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupConfig {
    /// 6-field cron expression for cleanup runs (e.g. "0 0 3 * * *" = 3am daily).
    pub schedule: Option<String>,
    /// Number of most-recent tags to keep per registry repository (default 5).
    #[serde(default = "default_registry_keep_tags")]
    pub registry_keep_tags: u32,
    /// Registry container name (default "orca-registry").
    #[serde(default = "default_registry_container")]
    pub registry_container: String,
}

fn default_registry_keep_tags() -> u32 {
    5
}

fn default_registry_container() -> String {
    "orca-registry".into()
}
