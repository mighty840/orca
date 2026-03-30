//! Raft consensus layer for multi-node cluster coordination.

pub mod api;
pub mod log_store;
pub mod network;
pub mod state_machine;
pub mod type_config;

use openraft::Raft;

use self::type_config::OrcaTypeConfig;

/// The concrete Raft node type used throughout the Orca control plane.
pub type OrcaRaft = Raft<OrcaTypeConfig>;
