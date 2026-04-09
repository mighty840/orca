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

/// Image pull policy for container services.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullPolicy {
    /// Pull for `:latest` tags, skip for pinned tags if present locally.
    #[default]
    Auto,
    /// Always pull, even if image exists locally.
    Always,
    /// Never pull; fail if not present locally.
    Never,
    /// Only pull if not present locally (skip for all tags including :latest).
    IfNotPresent,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_policy_default_is_auto() {
        assert_eq!(PullPolicy::default(), PullPolicy::Auto);
    }

    #[test]
    fn pull_policy_serde_roundtrip() {
        for (policy, expected) in [
            (PullPolicy::Auto, "\"auto\""),
            (PullPolicy::Always, "\"always\""),
            (PullPolicy::Never, "\"never\""),
            (PullPolicy::IfNotPresent, "\"ifnotpresent\""),
        ] {
            let json = serde_json::to_string(&policy).unwrap();
            assert_eq!(json, expected);
            let parsed: PullPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, policy);
        }
    }

    #[test]
    fn pull_policy_toml_roundtrip() {
        #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
        struct Wrapper {
            pull_policy: PullPolicy,
        }
        for policy in [
            PullPolicy::Auto,
            PullPolicy::Always,
            PullPolicy::Never,
            PullPolicy::IfNotPresent,
        ] {
            let w = Wrapper {
                pull_policy: policy,
            };
            let toml_str = toml::to_string(&w).unwrap();
            let parsed: Wrapper = toml::from_str(&toml_str).unwrap();
            assert_eq!(parsed, w);
        }
    }

    #[test]
    fn pull_policy_omitted_defaults_to_auto() {
        #[derive(Debug, serde::Deserialize, PartialEq)]
        struct Wrapper {
            #[serde(default)]
            pull_policy: PullPolicy,
        }
        let parsed: Wrapper = toml::from_str("").unwrap();
        assert_eq!(parsed.pull_policy, PullPolicy::Auto);
    }
}
