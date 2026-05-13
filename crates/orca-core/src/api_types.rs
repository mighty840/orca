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
    /// Project name (from service directory).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Current memory usage (e.g. "128Mi"), if stats are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_usage: Option<String>,
    /// Current CPU usage percentage, if stats are available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f64>,
    /// Node address where this service is currently running. `None` means
    /// the service is scheduled on the master. Populated from
    /// `service.placement.node` at report time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Configured memory limit in bytes, from
    /// `service.resources.memory`. Lets the TUI scale the memory
    /// sparkline against the real ceiling instead of auto-scaling to
    /// the sample peak.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
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

/// Response from `GET /api/v1/cluster/backups`. Aggregates backup status
/// across every node in the cluster for the TUI dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterBackupsResponse {
    pub nodes: Vec<NodeBackupStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeBackupStatus {
    /// `None` for the master, `Some(node_id)` for joined agents — saves the
    /// client from carrying a separate `is_master` flag.
    #[serde(default)]
    pub node_id: Option<u64>,
    pub hostname: String,
    pub role: NodeRole,
    pub snapshots: Vec<crate::backup::BackupSnapshotSummary>,
    /// Last `BackupResult` seen for this node, if any.
    #[serde(default)]
    pub last_result: Option<LastBackupResult>,
    /// `false` if the agent did not respond before the dashboard timeout.
    /// Master is always reachable in practice — we surface it here for the
    /// "did we get fresh data" indicator.
    pub reachable: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Master,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastBackupResult {
    pub success: bool,
    pub message: String,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

/// One push that matched this webhook config. Stored in an in-memory ring
/// buffer per service so the dashboard can show recent activity without us
/// having to scrape logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookInvocation {
    pub at: chrono::DateTime<chrono::Utc>,
    /// HTTP status code returned to the caller (200, 401, 503, etc.).
    pub status_code: u16,
    /// Short commit SHA (first 8 chars). Empty if the push payload had none.
    pub commit_sha: String,
    pub repo: String,
    pub branch: String,
    /// `true` if the redeploy/infra-deploy actually ran. `false` for ignored
    /// pushes (wrong branch, no matching config, signature mismatch).
    pub deployed: bool,
}

/// Augmented webhook record returned by `GET /api/v1/webhooks`. Wraps the
/// stored `WebhookConfig` with the most recent invocation so the dashboard
/// table renders without an extra round-trip per row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEntry {
    pub repo: String,
    pub service_name: String,
    pub branch: String,
    #[serde(default)]
    pub has_secret: bool,
    #[serde(default)]
    pub infra: bool,
    #[serde(default)]
    pub last_invocation: Option<WebhookInvocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookListResponse {
    pub webhooks: Vec<WebhookEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookInvocationsResponse {
    pub invocations: Vec<WebhookInvocation>,
}

/// Response from `POST /api/v1/cluster/backups/trigger`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerBackupResponse {
    /// Agent node IDs that received a `BackupRequest`.
    pub dispatched_to: Vec<u64>,
    /// `true` if the master's local backup was kicked off (runs as a
    /// subprocess in the background; result lands in
    /// `master_last_backup_result` when it completes).
    #[serde(default)]
    pub master_dispatched: bool,
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
                project: None,
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
                build: None,
                tls_cert: None,
                tls_key: None,
                internal: false,
                depends_on: vec![],
                cmd: vec![],
                extra_ports: vec![],
                strip_prefix: None,
                pull_policy: Default::default(),
                backup: None,
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: DeployRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.services.len(), 1);
        assert_eq!(back.services[0].name, "web");
    }
}
