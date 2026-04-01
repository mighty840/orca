mod ai;
mod cluster;
mod service;

use std::path::Path;

use crate::error::{OrcaError, Result};

// -- Re-exports --

pub use crate::backup::{BackupConfig, BackupTarget};
pub use ai::{AiAlertConfig, AiConfig, AlertDeliveryChannels, AutoRemediateConfig};
pub use cluster::NetworkConfig;
pub use cluster::{
    AlertChannelConfig, ClusterConfig, ClusterMeta, NodeConfig, NodeGpuConfig, ObservabilityConfig,
};
pub use service::{BuildConfig, ProbeConfig, ServiceConfig, ServicesConfig};

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

    /// Auto-discover services from subdirectories.
    ///
    /// Scans `dir/*/service.toml` and merges all service definitions.
    /// If a `secrets.json` exists in the same subdirectory, secret patterns
    /// in env vars (`${secrets.KEY}`) are resolved before returning.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut all_services = Vec::new();
        let entries = std::fs::read_dir(dir)
            .map_err(|e| OrcaError::Config(format!("failed to read {}: {e}", dir.display())))?;

        let mut subdirs: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        subdirs.sort_by_key(|e| e.file_name());

        for entry in subdirs {
            let svc_file = entry.path().join("service.toml");
            if svc_file.exists() {
                let mut config = Self::load(&svc_file)?;
                // Resolve secrets from per-service secrets.json
                let secrets_file = entry.path().join("secrets.json");
                if secrets_file.exists()
                    && let Ok(store) = crate::secrets::SecretStore::open(&secrets_file)
                {
                    for svc in &mut config.service {
                        svc.env = store.resolve_env(&svc.env);
                    }
                }
                all_services.extend(config.service);
            }
        }

        if all_services.is_empty() {
            return Err(OrcaError::Config(format!(
                "no service.toml files found in {}",
                dir.display()
            )));
        }

        Ok(ServicesConfig {
            service: all_services,
        })
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
    fn load_dir_discovers_services() {
        let dir = tempfile::tempdir().unwrap();
        // Create two service subdirs
        let svc_a = dir.path().join("alpha");
        let svc_b = dir.path().join("beta");
        std::fs::create_dir_all(&svc_a).unwrap();
        std::fs::create_dir_all(&svc_b).unwrap();

        std::fs::write(
            svc_a.join("service.toml"),
            r#"
[[service]]
name = "alpha"
image = "nginx:latest"
port = 80
"#,
        )
        .unwrap();

        std::fs::write(
            svc_b.join("service.toml"),
            r#"
[[service]]
name = "beta-db"
image = "postgres:16"
port = 5432

[[service]]
name = "beta-app"
image = "myapp:latest"
port = 3000
"#,
        )
        .unwrap();

        let config = ServicesConfig::load_dir(dir.path()).unwrap();
        assert_eq!(config.service.len(), 3);
        assert_eq!(config.service[0].name, "alpha");
        assert_eq!(config.service[1].name, "beta-db");
        assert_eq!(config.service[2].name, "beta-app");
    }

    #[test]
    fn load_dir_resolves_per_service_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let svc_dir = dir.path().join("myapp");
        std::fs::create_dir_all(&svc_dir).unwrap();

        std::fs::write(
            svc_dir.join("service.toml"),
            r#"
[[service]]
name = "myapp"
image = "myapp:latest"
port = 3000

[service.env]
DB_PASS = "${secrets.DB_PASS}"
PLAIN = "hello"
"#,
        )
        .unwrap();

        // Create secrets.json in the service dir
        let secrets_path = svc_dir.join("secrets.json");
        let mut store = crate::secrets::SecretStore::open(&secrets_path).unwrap();
        store.set("DB_PASS", "s3cret").unwrap();
        drop(store);

        let config = ServicesConfig::load_dir(dir.path()).unwrap();
        assert_eq!(config.service[0].env["DB_PASS"], "s3cret");
        assert_eq!(config.service[0].env["PLAIN"], "hello");
    }

    #[test]
    fn load_dir_errors_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let result = ServicesConfig::load_dir(dir.path());
        assert!(result.is_err());
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
