//! Shared application state for the control plane.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::runtime::{Runtime, WorkloadHandle};
use orca_core::types::{Replicas, WorkloadStatus};

// Re-export RouteTarget for convenience
pub use orca_proxy::RouteTarget;

/// Shared route table type, compatible with [`orca_proxy::run_proxy`].
pub type SharedRouteTable = Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>;

/// Shared state for the control plane, accessible by the API server and reconciler.
pub struct AppState {
    /// Cluster configuration.
    pub cluster_config: ClusterConfig,
    /// The container/wasm runtime.
    pub runtime: Arc<dyn Runtime>,
    /// Current service state, keyed by service name.
    pub services: RwLock<HashMap<String, ServiceState>>,
    /// Routing table shared with the reverse proxy.
    pub route_table: SharedRouteTable,
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
    /// Host port mapped to the container's primary port.
    pub host_port: Option<u16>,
}

impl AppState {
    /// Create a new state with a fresh route table.
    pub fn new(cluster_config: ClusterConfig, runtime: Arc<dyn Runtime>) -> Self {
        Self {
            cluster_config,
            runtime,
            services: RwLock::new(HashMap::new()),
            route_table: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with a shared route table (for sharing with the proxy).
    pub fn with_shared_routes(
        cluster_config: ClusterConfig,
        runtime: Arc<dyn Runtime>,
        route_table: SharedRouteTable,
    ) -> Self {
        Self {
            cluster_config,
            runtime,
            services: RwLock::new(HashMap::new()),
            route_table,
        }
    }
}

impl ServiceState {
    /// Create from a service config.
    pub fn from_config(config: ServiceConfig) -> Self {
        let desired_replicas = match &config.replicas {
            Replicas::Fixed(n) => *n,
            Replicas::Auto => 1, // Default to 1 in M0
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
