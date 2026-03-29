use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{OrcaError, Result};
use crate::types::{
    DeployStrategy, PlacementConstraint, Replicas, ResourceLimits, RuntimeKind, VolumeSpec,
};

/// Top-level cluster configuration (`cluster.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub cluster: ClusterMeta,
    #[serde(default)]
    pub node: Vec<NodeConfig>,
    #[serde(default)]
    pub observability: Option<ObservabilityConfig>,
    #[serde(default)]
    pub ai: Option<AiConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMeta {
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

fn default_log_level() -> String {
    "info".into()
}

fn default_api_port() -> u16 {
    6880
}

fn default_grpc_port() -> u16 {
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

fn default_gpu_count() -> u32 {
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

// -- AI Configuration --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// LLM provider: "litellm", "ollama", "openai", "anthropic"
    #[serde(default = "default_ai_provider")]
    pub provider: String,
    /// Endpoint URL (for litellm/ollama/compatible APIs).
    pub endpoint: Option<String>,
    /// Model identifier.
    pub model: Option<String>,
    /// API key (or use ${secrets.ai_api_key}).
    pub api_key: Option<String>,
    /// Conversational alerting configuration.
    #[serde(default)]
    pub alerts: Option<AiAlertConfig>,
    /// Auto-remediation rules.
    #[serde(default)]
    pub auto_remediate: Option<AutoRemediateConfig>,
}

fn default_ai_provider() -> String {
    "ollama".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAlertConfig {
    /// Enable conversational alerts (default: true when [ai] is configured).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// How often to analyze cluster health (seconds, default: 60).
    #[serde(default = "default_analysis_interval")]
    pub analysis_interval_secs: u64,
    /// Channels to deliver conversation updates.
    pub channels: Option<AlertDeliveryChannels>,
}

fn default_true() -> bool {
    true
}

fn default_analysis_interval_secs() -> u64 {
    60
}

fn default_analysis_interval() -> u64 {
    default_analysis_interval_secs()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertDeliveryChannels {
    /// Webhook URL for alert conversation updates.
    pub webhook: Option<String>,
    /// Slack webhook for threaded alert conversations.
    pub slack: Option<String>,
    /// Email for alert digests.
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoRemediateConfig {
    /// Auto-restart crashed services (default: true).
    #[serde(default = "default_true")]
    pub restart_crashed: bool,
    /// Auto-scale on resource pressure (default: false, suggest only).
    #[serde(default)]
    pub scale_on_pressure: bool,
    /// Auto-rollback on deploy failure (default: false, suggest only).
    #[serde(default)]
    pub rollback_on_failure: bool,
}

/// Services configuration (`services.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicesConfig {
    pub service: Vec<ServiceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    #[serde(default)]
    pub runtime: RuntimeKind,
    /// Container image (for container runtime).
    pub image: Option<String>,
    /// Wasm module path or OCI reference (for wasm runtime).
    pub module: Option<String>,
    #[serde(default)]
    pub replicas: Replicas,
    pub port: Option<u16>,
    pub domain: Option<String>,
    pub health: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub resources: Option<ResourceLimits>,
    pub volume: Option<VolumeSpec>,
    pub deploy: Option<DeployStrategy>,
    pub placement: Option<PlacementConstraint>,
    /// Wasm triggers: "http:/path", "cron:expr", "queue:topic", "event:pattern"
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Static assets directory (for builtin:static-server Wasm module).
    pub assets: Option<String>,
}

impl ClusterConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| OrcaError::Config(format!("failed to read {}: {e}", path.display())))?;
        toml::from_str(&content)
            .map_err(|e| OrcaError::Config(format!("failed to parse {}: {e}", path.display())))
    }
}

impl ServicesConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| OrcaError::Config(format!("failed to read {}: {e}", path.display())))?;
        toml::from_str(&content)
            .map_err(|e| OrcaError::Config(format!("failed to parse {}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cluster_config() {
        let toml = r#"
[cluster]
name = "test"
domain = "example.com"
acme_email = "ops@example.com"

[[node]]
address = "10.0.0.1"
labels = { zone = "eu-1" }

[[node]]
address = "10.0.0.2"
"#;
        let config: ClusterConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.cluster.name, "test");
        assert_eq!(config.node.len(), 2);
        assert_eq!(config.cluster.api_port, 6880);
    }

    #[test]
    fn parse_services_config() {
        let toml = r#"
[[service]]
name = "api"
image = "ghcr.io/myorg/api:latest"
replicas = 3
port = 8080
health = "/healthz"
domain = "api.example.com"

[service.env]
DATABASE_URL = "postgres://localhost/db"

[[service]]
name = "edge"
runtime = "wasm"
module = "./modules/edge.wasm"
triggers = ["http:/api/edge/*"]
"#;
        let config: ServicesConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.service.len(), 2);
        assert_eq!(config.service[0].name, "api");
        assert_eq!(config.service[1].runtime, RuntimeKind::Wasm);
    }

    #[test]
    fn parse_gpu_node_config() {
        let toml = r#"
[cluster]
name = "gpu-cluster"

[[node]]
address = "10.0.0.1"
labels = { role = "gpu" }

[[node.gpus]]
vendor = "nvidia"
count = 2
model = "A100"
"#;
        let config: ClusterConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.node[0].gpus.len(), 1);
        assert_eq!(config.node[0].gpus[0].vendor, "nvidia");
        assert_eq!(config.node[0].gpus[0].count, 2);
    }

    #[test]
    fn parse_gpu_service_config() {
        let toml = r#"
[[service]]
name = "llm-inference"
image = "vllm/vllm-openai:latest"
port = 8000

[service.resources]
memory = "32Gi"
cpu = 8.0

[service.resources.gpu]
count = 1
vendor = "nvidia"
vram_min = 40000
"#;
        let config: ServicesConfig = toml::from_str(toml).unwrap();
        let gpu = config.service[0].resources.as_ref().unwrap().gpu.as_ref().unwrap();
        assert_eq!(gpu.count, 1);
        assert_eq!(gpu.vendor.as_deref(), Some("nvidia"));
    }

    #[test]
    fn parse_ai_config() {
        let toml = r#"
[cluster]
name = "test"

[ai]
provider = "litellm"
endpoint = "https://llm.example.com"
model = "qwen3-30b"

[ai.alerts]
enabled = true
analysis_interval_secs = 30

[ai.auto_remediate]
restart_crashed = true
scale_on_pressure = false
"#;
        let config: ClusterConfig = toml::from_str(toml).unwrap();
        let ai = config.ai.as_ref().unwrap();
        assert_eq!(ai.provider, "litellm");
        assert_eq!(ai.alerts.as_ref().unwrap().analysis_interval_secs, 30);
        assert!(ai.auto_remediate.as_ref().unwrap().restart_crashed);
    }
}
