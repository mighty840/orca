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
fn parse_build_config() {
    let toml = r#"
[[service]]
name = "custom-api"
port = 3000
domain = "custom.example.com"

[service.build]
repo = "git@github.com:myorg/api.git"
branch = "main"
dockerfile = "Dockerfile"
context = "."
"#;
    let config: ServicesConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.service.len(), 1);
    let svc = &config.service[0];
    assert!(svc.image.is_none());
    let build = svc.build.as_ref().unwrap();
    assert_eq!(build.repo, "git@github.com:myorg/api.git");
    assert_eq!(build.branch_or_default(), "main");
    assert_eq!(build.dockerfile_or_default(), "Dockerfile");
    assert_eq!(build.context_or_default(), ".");
}

#[test]
fn parse_build_config_defaults() {
    let toml = r#"
[[service]]
name = "minimal-build"
port = 8080

[service.build]
repo = "https://github.com/org/repo.git"
"#;
    let config: ServicesConfig = toml::from_str(toml).unwrap();
    let build = config.service[0].build.as_ref().unwrap();
    assert!(build.branch.is_none());
    assert_eq!(build.branch_or_default(), "main");
    assert!(build.dockerfile.is_none());
    assert_eq!(build.dockerfile_or_default(), "Dockerfile");
    assert!(build.context.is_none());
    assert_eq!(build.context_or_default(), ".");
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
