//! Cluster-wide state backed by Raft consensus.
//!
//! Reads are served locally from the store. Writes are proposed
//! to the Raft leader for consensus.

use std::collections::HashMap;
use std::sync::Arc;

use openraft::BasicNode;
use tracing::info;

use orca_core::config::ServiceConfig;

use crate::raft::OrcaRaft;
use crate::store::{Assignment, ClusterStore, NodeEntry, RaftEntry};

/// Cluster state manager wrapping Raft consensus.
pub struct ClusterState {
    /// The Raft node.
    pub raft: Arc<OrcaRaft>,
    /// Local state store (reads served directly).
    pub store: Arc<ClusterStore>,
}

impl ClusterState {
    /// Create a new cluster state manager.
    pub fn new(raft: Arc<OrcaRaft>, store: Arc<ClusterStore>) -> Self {
        Self { raft, store }
    }

    // -- Read operations (local, no Raft) --

    /// Get all registered nodes.
    pub fn get_nodes(&self) -> anyhow::Result<HashMap<u64, NodeEntry>> {
        self.store.get_all_nodes()
    }

    /// Get all service configs.
    pub fn get_services(&self) -> anyhow::Result<HashMap<String, ServiceConfig>> {
        self.store.get_all_services()
    }

    /// Get all workload assignments.
    pub fn get_all_assignments(&self) -> anyhow::Result<Vec<Assignment>> {
        self.store.get_all_assignments()
    }

    /// Get assignments for a specific service.
    pub fn get_assignments(&self, service: &str) -> anyhow::Result<Vec<Assignment>> {
        self.store.get_assignments(service)
    }

    /// Get assignments for a specific node.
    pub fn get_node_assignments(&self, node_id: u64) -> anyhow::Result<Vec<Assignment>> {
        let all = self.store.get_all_assignments()?;
        Ok(all.into_iter().filter(|a| a.node_id == node_id).collect())
    }

    // -- Write operations (proposed to Raft leader) --

    /// Deploy services — proposes SetService entries to Raft.
    pub async fn propose_deploy(&self, services: &[ServiceConfig]) -> anyhow::Result<()> {
        for svc in services {
            self.propose(RaftEntry::SetService(Box::new(svc.clone())))
                .await?;
        }
        info!("Proposed deploy of {} services to Raft", services.len());
        Ok(())
    }

    /// Register a node in the cluster.
    pub async fn register_node(
        &self,
        node_id: u64,
        address: String,
        labels: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        // Add to Raft membership
        let node = BasicNode {
            addr: address.clone(),
        };
        let _ = self.raft.add_learner(node_id, node, true).await;

        // Register in cluster state
        self.propose(RaftEntry::RegisterNode {
            node_id,
            address,
            labels,
        })
        .await?;
        Ok(())
    }

    /// Assign a workload replica to a node.
    pub async fn assign_workload(
        &self,
        service: &str,
        replica_idx: u32,
        node_id: u64,
    ) -> anyhow::Result<()> {
        self.propose(RaftEntry::AssignWorkload {
            service: service.to_string(),
            replica_idx,
            node_id,
        })
        .await
    }

    /// Unassign a workload replica from a node.
    pub async fn unassign_workload(
        &self,
        service: &str,
        replica_idx: u32,
        node_id: u64,
    ) -> anyhow::Result<()> {
        self.propose(RaftEntry::UnassignWorkload {
            service: service.to_string(),
            replica_idx,
            node_id,
        })
        .await
    }

    /// Propose a Raft entry for consensus.
    async fn propose(&self, entry: RaftEntry) -> anyhow::Result<()> {
        self.raft
            .client_write(entry)
            .await
            .map_err(|e| anyhow::anyhow!("Raft propose failed: {e}"))?;
        Ok(())
    }
}
