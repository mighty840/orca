mod alert;
mod gpu;
mod node;
mod trigger;
mod workload;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// -- Identifiers --

pub type NodeId = Uuid;
pub type WorkloadId = Uuid;
pub type DeploymentId = Uuid;
pub type ConversationId = Uuid;

// -- Runtime --

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    #[default]
    Container,
    Wasm,
}

// -- Re-exports --

pub use alert::{AlertConversation, AlertMessage, AlertSender, AlertSeverity, AlertState};
pub use gpu::{GpuInfo, GpuSpec, GpuStats};
pub use node::{NodeInfo, NodeResources, NodeStatus};
pub use trigger::Trigger;
pub use workload::{
    DeployKind, DeployStrategy, HealthState, PlacementConstraint, Replicas, ResourceLimits,
    ResourceStats, VolumeSpec, WorkloadInstance, WorkloadSpec, WorkloadStatus,
};
