use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::gpu::GpuSpec;
use super::trigger::Trigger;
use super::{GpuStats, NodeId, RuntimeKind, WorkloadId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSpec {
    pub name: String,
    pub runtime: RuntimeKind,
    /// Container image (runtime = container) or Wasm module path/OCI ref (runtime = wasm)
    pub image: String,
    pub replicas: Replicas,
    /// Container port (internal).
    pub port: Option<u16>,
    /// Host port to bind (specific port on host, e.g. 443).
    pub host_port: Option<u16>,
    pub domain: Option<String>,
    /// Path routes under the domain (e.g., ["/api/*"]).
    pub routes: Vec<String>,
    pub health: Option<String>,
    /// Readiness probe config.
    pub readiness: Option<crate::config::ProbeConfig>,
    /// Liveness probe config.
    pub liveness: Option<crate::config::ProbeConfig>,
    pub env: std::collections::HashMap<String, String>,
    pub resources: Option<ResourceLimits>,
    pub volume: Option<VolumeSpec>,
    pub deploy: Option<DeployStrategy>,
    pub placement: Option<PlacementConstraint>,
    /// Docker network name (auto-prefixed with "orca-").
    pub network: Option<String>,
    /// Network aliases (resolvable names within the Docker network).
    pub aliases: Vec<String>,
    /// Host bind mounts (e.g., ["/host/path:/container/path:ro"]).
    pub mounts: Vec<String>,
    pub triggers: Vec<Trigger>,
}

/// Replica count: either a fixed number or "auto" for auto-scaling.
#[derive(Debug, Clone)]
pub enum Replicas {
    Fixed(u32),
    Auto,
}

impl serde::Serialize for Replicas {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Replicas::Fixed(n) => serializer.serialize_u32(*n),
            Replicas::Auto => serializer.serialize_str("auto"),
        }
    }
}

impl Default for Replicas {
    fn default() -> Self {
        Self::Fixed(1)
    }
}

impl<'de> serde::Deserialize<'de> for Replicas {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct ReplicasVisitor;

        impl<'de> de::Visitor<'de> for ReplicasVisitor {
            type Value = Replicas;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a positive integer or the string \"auto\"")
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Replicas, E> {
                Ok(Replicas::Fixed(v as u32))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Replicas, E> {
                if v >= 0 {
                    Ok(Replicas::Fixed(v as u32))
                } else {
                    Err(E::custom("replicas must be non-negative"))
                }
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Replicas, E> {
                if v == "auto" {
                    Ok(Replicas::Auto)
                } else {
                    Err(E::custom(format!(
                        "expected \"auto\" or a number, got \"{v}\""
                    )))
                }
            }
        }

        deserializer.deserialize_any(ReplicasVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub memory: Option<String>,
    pub cpu: Option<f64>,
    /// GPU requirements. If set, scheduler places workload on GPU-equipped nodes.
    pub gpu: Option<GpuSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub path: String,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStrategy {
    #[serde(default = "default_strategy")]
    pub strategy: DeployKind,
    pub max_unavailable: Option<u32>,
}

fn default_strategy() -> DeployKind {
    DeployKind::Rolling
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeployKind {
    Rolling,
    BlueGreen,
    Canary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementConstraint {
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub node: Option<String>,
    /// Require GPU-equipped node.
    pub requires_gpu: Option<bool>,
}

// -- Workload Instance --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadInstance {
    pub id: WorkloadId,
    pub spec_name: String,
    pub node_id: NodeId,
    pub runtime: RuntimeKind,
    pub status: WorkloadStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub health: HealthState,
    /// GPU device indices assigned to this workload.
    pub gpu_devices: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkloadStatus {
    Pending,
    Creating,
    Running,
    Stopping,
    Stopped,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    Unknown,
    Healthy,
    Unhealthy,
    NoCheck,
}

// -- Resource Stats --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceStats {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub gpu_stats: Vec<GpuStats>,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replicas_default_is_fixed_one() {
        let r = Replicas::default();
        assert!(matches!(r, Replicas::Fixed(1)));
    }

    #[test]
    fn runtime_kind_default_is_container() {
        let k = RuntimeKind::default();
        assert!(matches!(k, RuntimeKind::Container));
    }
}
