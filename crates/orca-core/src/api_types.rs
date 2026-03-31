//! Shared request/response types for the Orca REST API.
//!
//! Used by both the control plane (server) and CLI (client).

use serde::{Deserialize, Serialize};

use crate::config::ServiceConfig;
use crate::types::RuntimeKind;

/// Request body for `POST /api/v1/deploy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRequest {
    /// Service definitions to deploy.
    pub services: Vec<ServiceConfig>,
}

/// Response from `POST /api/v1/deploy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployResponse {
    /// Names of services that were successfully deployed.
    pub deployed: Vec<String>,
    /// Errors encountered during deployment.
    pub errors: Vec<String>,
}

/// Response from `GET /api/v1/status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Cluster name from config.
    pub cluster_name: String,
    /// Status of each service.
    pub services: Vec<ServiceStatus>,
}

/// Status summary for a single service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// Service name.
    pub name: String,
    /// Container image or Wasm module reference.
    pub image: String,
    /// Runtime type.
    pub runtime: RuntimeKind,
    /// Number of replicas requested.
    pub desired_replicas: u32,
    /// Number of replicas currently running.
    pub running_replicas: u32,
    /// Overall status string (e.g., "running", "degraded", "stopped").
    pub status: String,
    /// Domain for external access, if configured.
    pub domain: Option<String>,
}

/// Request body for `POST /api/v1/services/{name}/scale`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaleRequest {
    /// Desired number of replicas.
    pub replicas: u32,
}

/// Response from `POST /api/v1/services/{name}/scale`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaleResponse {
    /// Service name.
    pub service: String,
    /// New replica count.
    pub replicas: u32,
}

/// Query parameters for `GET /api/v1/services/{name}/logs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsQuery {
    /// Number of recent log lines to return.
    #[serde(default = "default_tail")]
    pub tail: u64,
    /// Whether to follow (stream) logs.
    #[serde(default)]
    pub follow: bool,
}

fn default_tail() -> u64 {
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_query_default_tail_is_100() {
        let q: LogsQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(q.tail, 100);
        assert!(!q.follow);
    }

    #[test]
    fn deploy_request_serialization_roundtrip() {
        let req = DeployRequest {
            services: vec![ServiceConfig {
                name: "web".into(),
                runtime: RuntimeKind::Container,
                image: Some("nginx:latest".into()),
                module: None,
                replicas: crate::types::Replicas::Fixed(2),
                port: Some(80),
                domain: Some("example.com".into()),
                health: Some("/healthz".into()),
                readiness: None,
                liveness: None,
                env: Default::default(),
                resources: None,
                volume: None,
                deploy: None,
                placement: None,
                network: None,
                aliases: vec![],
                mounts: vec![],
                routes: vec![],
                host_port: None,
                triggers: vec![],
                assets: None,
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: DeployRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.services.len(), 1);
        assert_eq!(back.services[0].name, "web");
    }
}
