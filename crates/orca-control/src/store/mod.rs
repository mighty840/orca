//! Persistent cluster store backed by redb.

mod kv;
mod restore;
mod types;

pub use kv::ClusterStore;
pub use types::{Assignment, NodeEntry, RaftEntry, RaftSnapshot};

/// Open the master's store, or explain why the master must not start.
///
/// Starting without it meant empty state: every route dropped, every
/// container recreated, paused services started, and nothing written back,
/// so the next restart did it all again (#179). The usual cause is a second
/// orca process holding the database lock.
pub fn open_or_refuse(path: &std::path::Path) -> anyhow::Result<ClusterStore> {
    ClusterStore::open(path).map_err(|e| {
        anyhow::anyhow!(
            "cannot open the cluster store {}: {e}. If another orca process is running \
             (`pgrep -a orca`), stop it first. Refusing to start with empty state, which \
             would drop every route, recreate every container and start paused services.",
            path.display()
        )
    })
}

#[cfg(test)]
#[path = "open_tests.rs"]
mod open_tests;
