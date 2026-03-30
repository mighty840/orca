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
