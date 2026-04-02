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
    AlertChannelConfig, ApiToken, ClusterConfig, ClusterMeta, NodeConfig, NodeGpuConfig,
    ObservabilityConfig, Role,
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
                let project_name = entry.file_name().to_string_lossy().to_string();

                // Resolve secrets from per-service secrets.json
                let secrets_file = entry.path().join("secrets.json");
                if secrets_file.exists()
                    && let Ok(store) = crate::secrets::SecretStore::open(&secrets_file)
                {
                    for svc in &mut config.service {
                        svc.env = store.resolve_env(&svc.env);
                    }
                }

                // Set project name and default network from directory
                for svc in &mut config.service {
                    svc.project = Some(project_name.clone());
                    if svc.network.is_none() {
                        svc.network = Some(project_name.clone());
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
#[path = "tests_parse.rs"]
mod tests_parse;

#[cfg(test)]
#[path = "tests_load.rs"]
mod tests_load;
