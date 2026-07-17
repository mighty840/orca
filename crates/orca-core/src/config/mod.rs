mod ai;
mod cluster;
mod service;

use std::path::Path;

use crate::error::{OrcaError, Result};

// -- Re-exports --

pub use crate::backup::{BackupConfig, BackupTarget};
pub use ai::{
    AiAlertConfig, AiConfig, AlertDeliveryChannels, AutoRemediateConfig, EmailChannelConfig,
    SmtpTls,
};
pub use cluster::NetworkConfig;
pub use cluster::{
    AlertChannelConfig, ApiToken, CleanupConfig, ClusterConfig, ClusterMeta, DeployConfig,
    FallbackConfig, NodeConfig, NodeGpuConfig, ObservabilityConfig, ReconcileConfig, Role,
    SecretsConfig, SecurityHeadersConfig,
};
pub use service::{BuildConfig, ProbeConfig, ServiceConfig, ServicesConfig};

// -- Load methods --

impl ClusterConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| OrcaError::Config(format!("failed to read {}: {e}", path.display())))?;
        let mut config: Self = toml::from_str(&content)
            .map_err(|e| OrcaError::Config(format!("failed to parse {}: {e}", path.display())))?;
        // Install the encrypted-secrets backend BEFORE resolving
        // `${secrets.X}` fields below, so they resolve against the store
        // `[secrets]` selects (#109).
        if let Some(secrets_cfg) = &config.secrets {
            let base_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            crate::secrets::configure(secrets_cfg, base_dir);
        }
        config.resolve_secrets();
        Ok(config)
    }

    /// Resolve `${secrets.X}` patterns in config fields that may contain secrets.
    fn resolve_secrets(&mut self) {
        let store = match crate::secrets::open_configured() {
            Ok(s) => s,
            Err(_) => return,
        };
        let resolve = |s: &mut Option<String>| {
            if let Some(val) = s
                && val.contains("${secrets.")
            {
                let as_map = std::collections::HashMap::from([("_".to_string(), val.clone())]);
                let resolved = store.resolve_env(&as_map);
                *val = resolved.get("_").cloned().unwrap_or_default();
            }
        };
        // AI config
        if let Some(ai) = &mut self.ai {
            resolve(&mut ai.api_key);
            resolve(&mut ai.endpoint);
            if let Some(alerts) = &mut ai.alerts
                && let Some(channels) = &mut alerts.channels
                && let Some(email) = &mut channels.email
            {
                let mut p = Some(email.password.clone());
                resolve(&mut p);
                if let Some(resolved) = p {
                    email.password = resolved;
                }
            }
        }
        // Network config
        if let Some(net) = &mut self.network {
            resolve(&mut net.setup_key);
        }
        // Named API tokens
        for token in &mut self.token {
            let mut v = Some(token.value.clone());
            resolve(&mut v);
            if let Some(resolved) = v {
                token.value = resolved;
            }
        }
        // Backup S3 credentials
        if let Some(backup) = &mut self.backup {
            for target in &mut backup.targets {
                if let BackupTarget::S3 {
                    access_key,
                    secret_key,
                    ..
                } = target
                {
                    resolve(access_key);
                    resolve(secret_key);
                }
            }
        }
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

                // Secrets are resolved later in service_config_to_spec()
                // so that spec_matches() compares unresolved templates,
                // preventing unnecessary restarts when token values change.

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

#[cfg(test)]
#[path = "tests_secrets.rs"]
mod tests_secrets;
