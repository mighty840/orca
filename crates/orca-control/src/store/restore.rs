//! Snapshot restore logic for `ClusterStore`.

use std::collections::HashMap;

use tracing::debug;

use super::kv::{ASSIGNMENTS, ClusterStore, NODES, SERVICES};
use super::types::{Assignment, RaftSnapshot};

impl ClusterStore {
    /// Restore the store from a snapshot, replacing all data atomically.
    pub fn restore_from_snapshot(&self, snap: &RaftSnapshot) -> anyhow::Result<()> {
        let tx = self.db.begin_write()?;
        let mut nt = tx.open_table(NODES)?;
        while nt.pop_last()?.is_some() {}
        for (id, n) in &snap.nodes {
            nt.insert(*id, serde_json::to_vec(n)?.as_slice())?;
        }
        drop(nt);
        let mut st = tx.open_table(SERVICES)?;
        while st.pop_last()?.is_some() {}
        for (name, c) in &snap.services {
            st.insert(name.as_str(), serde_json::to_vec(c)?.as_slice())?;
        }
        drop(st);
        let mut at = tx.open_table(ASSIGNMENTS)?;
        while at.pop_last()?.is_some() {}
        let mut by_svc: HashMap<&str, Vec<&Assignment>> = HashMap::new();
        for a in &snap.assignments {
            by_svc.entry(&a.service).or_default().push(a);
        }
        for (s, v) in &by_svc {
            at.insert(*s, serde_json::to_vec(&v)?.as_slice())?;
        }
        drop(at);
        tx.commit()?;
        debug!("Restored store from snapshot");
        Ok(())
    }
}
