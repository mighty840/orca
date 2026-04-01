//! Key-value cluster store backed by redb.

use std::collections::HashMap;
use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition};
use tracing::debug;

use orca_core::config::ServiceConfig;
use orca_core::types::NodeStatus;

use super::types::{Assignment, NodeEntry, RaftEntry, RaftSnapshot};

pub(super) const NODES: TableDefinition<u64, &[u8]> = TableDefinition::new("nodes");
pub(super) const SERVICES: TableDefinition<&str, &[u8]> = TableDefinition::new("services");
pub(super) const ASSIGNMENTS: TableDefinition<&str, &[u8]> = TableDefinition::new("assignments");

/// Persistent cluster state store.
pub struct ClusterStore {
    pub(super) db: Database,
}

impl ClusterStore {
    /// Open or create a store at the given path.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let db = Database::create(path)?;
        // Ensure tables exist
        let tx = db.begin_write()?;
        {
            let _ = tx.open_table(NODES)?;
            let _ = tx.open_table(SERVICES)?;
            let _ = tx.open_table(ASSIGNMENTS)?;
        }
        tx.commit()?;
        Ok(Self { db })
    }

    /// Apply a Raft log entry to the store.
    pub fn apply(&self, entry: &RaftEntry) -> anyhow::Result<()> {
        match entry {
            RaftEntry::RegisterNode {
                node_id,
                address,
                labels,
            } => {
                let node = NodeEntry {
                    node_id: *node_id,
                    address: address.clone(),
                    labels: labels.clone(),
                    status: NodeStatus::Ready,
                    resources: None,
                };
                self.set_node(*node_id, &node)?;
                debug!("Registered node {node_id} at {address}");
            }
            RaftEntry::DeregisterNode { node_id } => {
                self.remove_node(*node_id)?;
                debug!("Deregistered node {node_id}");
            }
            RaftEntry::SetService(config) => {
                self.set_service(&config.name, config)?;
                debug!("Set service {}", config.name);
            }
            RaftEntry::RemoveService(name) => {
                self.remove_service(name)?;
                debug!("Removed service {name}");
            }
            RaftEntry::AssignWorkload {
                service,
                replica_idx,
                node_id,
            } => {
                let mut assignments = self.get_assignments(service)?;
                assignments.push(Assignment {
                    service: service.clone(),
                    replica_idx: *replica_idx,
                    node_id: *node_id,
                });
                self.set_assignments(service, &assignments)?;
            }
            RaftEntry::UnassignWorkload {
                service,
                replica_idx,
                node_id,
            } => {
                let mut assignments = self.get_assignments(service)?;
                assignments.retain(|a| !(a.replica_idx == *replica_idx && a.node_id == *node_id));
                self.set_assignments(service, &assignments)?;
            }
            RaftEntry::UpdateNodeStatus {
                node_id,
                status,
                resources,
            } => {
                if let Ok(Some(mut node)) = self.get_node(*node_id) {
                    node.status = *status;
                    node.resources = Some(resources.clone());
                    self.set_node(*node_id, &node)?;
                }
            }
        }
        Ok(())
    }

    /// Get a node by ID.
    pub fn get_node(&self, id: u64) -> anyhow::Result<Option<NodeEntry>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(NODES)?;
        match table.get(id)? {
            Some(val) => Ok(Some(serde_json::from_slice(val.value())?)),
            None => Ok(None),
        }
    }

    fn set_node(&self, id: u64, node: &NodeEntry) -> anyhow::Result<()> {
        let data = serde_json::to_vec(node)?;
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(NODES)?;
            table.insert(id, data.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }

    fn remove_node(&self, id: u64) -> anyhow::Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(NODES)?;
            table.remove(id)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Get all registered nodes.
    pub fn get_all_nodes(&self) -> anyhow::Result<HashMap<u64, NodeEntry>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(NODES)?;
        let mut nodes = HashMap::new();
        for entry in table.iter()? {
            let (k, v) = entry?;
            let node: NodeEntry = serde_json::from_slice(v.value())?;
            nodes.insert(k.value(), node);
        }
        Ok(nodes)
    }

    /// Get a service config by name.
    pub fn get_service(&self, name: &str) -> anyhow::Result<Option<ServiceConfig>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SERVICES)?;
        match table.get(name)? {
            Some(val) => Ok(Some(serde_json::from_slice(val.value())?)),
            None => Ok(None),
        }
    }

    fn set_service(&self, name: &str, config: &ServiceConfig) -> anyhow::Result<()> {
        let data = serde_json::to_vec(config)?;
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(SERVICES)?;
            table.insert(name, data.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }

    fn remove_service(&self, name: &str) -> anyhow::Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(SERVICES)?;
            table.remove(name)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Get all service configs.
    pub fn get_all_services(&self) -> anyhow::Result<HashMap<String, ServiceConfig>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SERVICES)?;
        let mut services = HashMap::new();
        for entry in table.iter()? {
            let (k, v) = entry?;
            let config: ServiceConfig = serde_json::from_slice(v.value())?;
            services.insert(k.value().to_string(), config);
        }
        Ok(services)
    }

    /// Get assignments for a service.
    pub fn get_assignments(&self, service: &str) -> anyhow::Result<Vec<Assignment>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(ASSIGNMENTS)?;
        match table.get(service)? {
            Some(val) => Ok(serde_json::from_slice(val.value())?),
            None => Ok(Vec::new()),
        }
    }

    fn set_assignments(&self, service: &str, assignments: &[Assignment]) -> anyhow::Result<()> {
        let data = serde_json::to_vec(assignments)?;
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(ASSIGNMENTS)?;
            table.insert(service, data.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Take a full snapshot of the store.
    pub fn snapshot(&self) -> anyhow::Result<RaftSnapshot> {
        Ok(RaftSnapshot {
            nodes: self.get_all_nodes()?,
            services: self.get_all_services()?,
            assignments: self.get_all_assignments()?,
        })
    }

    /// Get all assignments across all services.
    pub fn get_all_assignments(&self) -> anyhow::Result<Vec<Assignment>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(ASSIGNMENTS)?;
        let mut all = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            let assignments: Vec<Assignment> = serde_json::from_slice(v.value())?;
            all.extend(assignments);
        }
        Ok(all)
    }
}

#[cfg(test)]
#[path = "kv_tests.rs"]
mod tests;
