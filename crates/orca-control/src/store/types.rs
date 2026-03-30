//! Types for the Raft state machine and cluster store.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use orca_core::config::ServiceConfig;
use orca_core::types::{NodeResources, NodeStatus};

/// A single Raft log entry — a mutation to cluster state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaftEntry {
    /// Register a new node in the cluster.
    RegisterNode {
        node_id: u64,
        address: String,
        labels: HashMap<String, String>,
    },
    /// Remove a node from the cluster.
    DeregisterNode { node_id: u64 },
    /// Add or update a service definition.
    SetService(Box<ServiceConfig>),
    /// Remove a service.
    RemoveService(String),
    /// Assign a workload replica to a node.
    AssignWorkload {
        service: String,
        replica_idx: u32,
        node_id: u64,
    },
    /// Unassign a workload replica from a node.
    UnassignWorkload {
        service: String,
        replica_idx: u32,
        node_id: u64,
    },
    /// Update a node's status and resource info.
    UpdateNodeStatus {
        node_id: u64,
        status: NodeStatus,
        resources: NodeResources,
    },
}

/// A workload assignment: which node runs which replica.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    /// Service name.
    pub service: String,
    /// Replica index within the service.
    pub replica_idx: u32,
    /// Node running this replica.
    pub node_id: u64,
}

/// Complete snapshot of cluster state for Raft snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftSnapshot {
    /// All registered nodes.
    pub nodes: HashMap<u64, NodeEntry>,
    /// All service configs.
    pub services: HashMap<String, ServiceConfig>,
    /// All workload assignments.
    pub assignments: Vec<Assignment>,
}

/// A registered node's info in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeEntry {
    /// Node's Raft ID.
    pub node_id: u64,
    /// Node's gRPC address (ip:port).
    pub address: String,
    /// Labels for scheduling (e.g., zone, role, gpu).
    pub labels: HashMap<String, String>,
    /// Current status.
    pub status: NodeStatus,
    /// Last known resource usage.
    pub resources: Option<NodeResources>,
}
