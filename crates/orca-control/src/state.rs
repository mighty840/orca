//! Shared application state for the control plane.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::runtime::{Runtime, WorkloadHandle};
use orca_core::types::{Replicas, WorkloadStatus};

pub use orca_proxy::{RouteTarget, SharedWasmTriggers, WasmTrigger};

/// Shared route table type, compatible with [`orca_proxy::run_proxy`].
pub type SharedRouteTable = Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>;

/// Shared state for the control plane, accessible by the API server and reconciler.
pub struct AppState {
    /// Cluster configuration.
    pub cluster_config: ClusterConfig,
    /// Container runtime (Docker).
    pub container_runtime: Arc<dyn Runtime>,
    /// Wasm runtime (wasmtime).
    pub wasm_runtime: Option<Arc<orca_agent::wasm::WasmRuntime>>,
    /// Current service state, keyed by service name.
    pub services: RwLock<HashMap<String, ServiceState>>,
    /// Routing table for container workloads, shared with the reverse proxy.
    pub route_table: SharedRouteTable,
    /// Wasm HTTP triggers, shared with the reverse proxy.
    pub wasm_triggers: SharedWasmTriggers,
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
}

impl AppState {
    /// Create with shared route table and Wasm triggers (for sharing with the proxy).
    pub fn new(
        cluster_config: ClusterConfig,
        container_runtime: Arc<dyn Runtime>,
        wasm_runtime: Option<Arc<orca_agent::wasm::WasmRuntime>>,
        route_table: SharedRouteTable,
        wasm_triggers: SharedWasmTriggers,
    ) -> Self {
        Self {
            cluster_config,
            container_runtime,
            wasm_runtime,
            services: RwLock::new(HashMap::new()),
            route_table,
            wasm_triggers,
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
