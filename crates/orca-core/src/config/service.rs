use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::{
    DeployStrategy, PlacementConstraint, Replicas, ResourceLimits, RuntimeKind, VolumeSpec,
};

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
    /// Health check path (e.g., "/healthz").
    pub health: Option<String>,
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
