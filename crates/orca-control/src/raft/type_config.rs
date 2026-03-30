//! Raft type configuration for the Orca cluster.

use std::io::Cursor;

use crate::store::RaftEntry;

openraft::declare_raft_types!(
    pub OrcaTypeConfig:
        D            = RaftEntry,
        R            = (),
        NodeId       = u64,
        Node         = openraft::BasicNode,
        Entry        = openraft::Entry<OrcaTypeConfig>,
        SnapshotData = Cursor<Vec<u8>>,
        AsyncRuntime = openraft::TokioRuntime,
);
