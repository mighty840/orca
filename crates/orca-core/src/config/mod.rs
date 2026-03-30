mod ai;
mod cluster;
mod service;

use std::path::Path;

use crate::error::{OrcaError, Result};

// -- Re-exports --

pub use ai::{AiAlertConfig, AiConfig, AlertDeliveryChannels, AutoRemediateConfig};
pub use cluster::{
    AlertChannelConfig, ClusterConfig, ClusterMeta, NodeConfig, NodeGpuConfig, ObservabilityConfig,
};
pub use service::{ServiceConfig, ServicesConfig};

// -- Load methods --

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
    use crate::types::RuntimeKind;

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
        let gpu = config.service[0]
            .resources
            .as_ref()
            .unwrap()
            .gpu
            .as_ref()
            .unwrap();
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
