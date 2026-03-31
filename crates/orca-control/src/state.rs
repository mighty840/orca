//! Shared application state for the control plane.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::runtime::{Runtime, WorkloadHandle};
use orca_core::types::{HealthState, Replicas, WorkloadStatus};

use crate::webhook::WebhookStore;

pub use orca_proxy::{RouteTarget, SharedWasmTriggers, WasmTrigger};

/// Shared route table type, compatible with [`orca_proxy::run_proxy`].
pub type SharedRouteTable = Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>;

/// Shared state for the control plane, accessible by the API server and reconciler.
pub struct AppState {
    /// Cluster configuration.
    pub cluster_config: ClusterConfig,
    /// Container runtime (Docker).
    pub container_runtime: Arc<dyn Runtime>,
    /// Wasm runtime (wasmtime). Trait object to avoid coupling to concrete type.
    pub wasm_runtime: Option<Arc<dyn Runtime>>,
    /// Current service state, keyed by service name.
    pub services: RwLock<HashMap<String, ServiceState>>,
    /// Routing table for container workloads, shared with the reverse proxy.
    pub route_table: SharedRouteTable,
    /// Wasm HTTP triggers, shared with the reverse proxy.
    pub wasm_triggers: SharedWasmTriggers,
    /// Registered cluster nodes (M2 in-memory, will move to Raft store).
    pub registered_nodes: RwLock<HashMap<u64, RegisteredNode>>,
    /// Webhook configurations for push-triggered deploys.
    pub webhooks: WebhookStore,
    /// API bearer tokens for authentication (empty = allow all).
    pub api_tokens: Vec<String>,
    /// Deploy history for rollback support.
    pub deploy_history: RwLock<crate::deploy_history::DeployHistory>,
}

/// A node registered in the cluster.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RegisteredNode {
    /// Node ID.
    pub node_id: u64,
    /// Node address (ip:port).
    pub address: String,
    /// Node labels.
    pub labels: HashMap<String, String>,
    /// Last heartbeat time.
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
}

/// State of a deployed service.
#[derive(Debug)]
pub struct ServiceState {
    /// The service configuration.
    pub config: ServiceConfig,
    /// Desired number of replicas.
    pub desired_replicas: u32,
    /// Running instances.
    pub instances: Vec<InstanceState>,
}

/// State of a single workload instance (one replica).
#[derive(Debug)]
pub struct InstanceState {
    /// Handle to the running workload.
    pub handle: WorkloadHandle,
    /// Current status.
    pub status: WorkloadStatus,
    /// Host port mapped to the container's primary port (containers only).
    pub host_port: Option<u16>,
    /// Health check state.
    pub health: HealthState,
}

impl AppState {
    /// Create with shared route table and Wasm triggers (for sharing with the proxy).
    pub fn new(
        cluster_config: ClusterConfig,
        container_runtime: Arc<dyn Runtime>,
        wasm_runtime: Option<Arc<dyn Runtime>>,
        route_table: SharedRouteTable,
        wasm_triggers: SharedWasmTriggers,
    ) -> Self {
        let api_tokens = cluster_config.api_tokens.clone();
        Self {
            cluster_config,
            container_runtime,
            wasm_runtime,
            services: RwLock::new(HashMap::new()),
            route_table,
            wasm_triggers,
            registered_nodes: RwLock::new(HashMap::new()),
            webhooks: crate::webhook::new_store(),
            api_tokens,
            deploy_history: RwLock::new(crate::deploy_history::DeployHistory::new()),
        }
    }
}

impl ServiceState {
    /// Create from a service config.
    pub fn from_config(config: ServiceConfig) -> Self {
        let desired_replicas = match &config.replicas {
            Replicas::Fixed(n) => *n,
            Replicas::Auto => 1,
        };
        Self {
            config,
            desired_replicas,
            instances: Vec::new(),
        }
    }

    /// Count how many instances are currently running.
    pub fn running_count(&self) -> u32 {
        self.instances
            .iter()
            .filter(|i| i.status == WorkloadStatus::Running)
            .count() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use orca_core::config::ServiceConfig;
    use orca_core::runtime::WorkloadHandle;
    use orca_core::types::{Replicas, WorkloadStatus};

    fn minimal_config(replicas: Replicas) -> ServiceConfig {
        ServiceConfig {
            name: "test-svc".to_string(),
            runtime: Default::default(),
            image: Some("nginx:latest".to_string()),
            module: None,
            replicas,
            port: Some(8080),
            domain: None,
            health: None,
            env: HashMap::new(),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: None,
            aliases: vec![],
            mounts: vec![],
            routes: vec![],
            host_port: None,
            triggers: Vec::new(),
            assets: None,
        }
    }

    fn make_instance(status: WorkloadStatus) -> InstanceState {
        InstanceState {
            handle: WorkloadHandle {
                runtime_id: "test-id".to_string(),
                name: "test-instance".to_string(),
                metadata: HashMap::new(),
            },
            status,
            host_port: None,
            health: HealthState::Unknown,
        }
    }

    #[test]
    fn from_config_fixed_sets_desired_replicas() {
        let state = ServiceState::from_config(minimal_config(Replicas::Fixed(3)));
        assert_eq!(state.desired_replicas, 3);
    }

    #[test]
    fn from_config_auto_defaults_to_one() {
        let state = ServiceState::from_config(minimal_config(Replicas::Auto));
        assert_eq!(state.desired_replicas, 1);
    }

    #[test]
    fn running_count_with_mixed_statuses() {
        let mut state = ServiceState::from_config(minimal_config(Replicas::Fixed(4)));
        state.instances = vec![
            make_instance(WorkloadStatus::Running),
            make_instance(WorkloadStatus::Stopped),
            make_instance(WorkloadStatus::Running),
            make_instance(WorkloadStatus::Failed),
        ];
        assert_eq!(state.running_count(), 2);
    }
}
