use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::{
    DeployStrategy, PlacementConstraint, Replicas, ResourceLimits, RuntimeKind, VolumeSpec,
};

/// Probe configuration for readiness/liveness checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeConfig {
    /// HTTP path to probe (e.g., "/healthz").
    pub path: String,
    /// Port to probe (defaults to service port).
    pub port: Option<u16>,
    /// Seconds between probes (default: 10).
    #[serde(default = "default_probe_interval")]
    pub interval_secs: u64,
    /// Seconds to wait for response (default: 3).
    #[serde(default = "default_probe_timeout")]
    pub timeout_secs: u64,
    /// Failures before action (default: 3).
    #[serde(default = "default_probe_failures")]
    pub failure_threshold: u32,
    /// Seconds to wait before first probe (default: 5).
    #[serde(default = "default_initial_delay")]
    pub initial_delay_secs: u64,
}

fn default_probe_interval() -> u64 {
    10
}
fn default_probe_timeout() -> u64 {
    3
}
fn default_probe_failures() -> u32 {
    3
}
fn default_initial_delay() -> u64 {
    5
}

/// Services configuration (`services.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicesConfig {
    pub service: Vec<ServiceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    #[serde(default)]
    pub runtime: RuntimeKind,
    /// Container image (for container runtime).
    pub image: Option<String>,
    /// Wasm module path or OCI reference (for wasm runtime).
    pub module: Option<String>,
    #[serde(default)]
    pub replicas: Replicas,
    /// Container port (internal).
    pub port: Option<u16>,
    /// Host port to bind (e.g., 443 for edge proxies). If omitted, ephemeral.
    pub host_port: Option<u16>,
    /// Domain for reverse proxy routing (orca proxy handles TLS).
    pub domain: Option<String>,
    /// Path routes under the domain (e.g., ["/api/*", "/admin/*"]).
    /// Default: ["/*"] (catch-all).
    #[serde(default)]
    pub routes: Vec<String>,
    /// Health check path (e.g., "/healthz"). Legacy shorthand for liveness probe.
    pub health: Option<String>,
    /// Readiness probe: container must pass before receiving traffic.
    pub readiness: Option<ProbeConfig>,
    /// Liveness probe: container is restarted if this fails.
    pub liveness: Option<ProbeConfig>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub resources: Option<ResourceLimits>,
    pub volume: Option<VolumeSpec>,
    pub deploy: Option<DeployStrategy>,
    pub placement: Option<PlacementConstraint>,
    /// Docker network name. Services with the same network can reach each other.
    /// Auto-prefixed with "orca-". If omitted, derived from service name prefix.
    pub network: Option<String>,
    /// Network aliases (resolvable names within the Docker network).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Host bind mounts (e.g., ["/host/path:/container/path:ro"]).
    #[serde(default)]
    pub mounts: Vec<String>,
    /// Wasm triggers: "http:/path", "cron:expr", "queue:topic", "event:pattern"
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Static assets directory (for builtin:static-server Wasm module).
    pub assets: Option<String>,
}
