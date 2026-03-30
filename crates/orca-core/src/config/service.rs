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
    pub port: Option<u16>,
    pub domain: Option<String>,
    pub health: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub resources: Option<ResourceLimits>,
    pub volume: Option<VolumeSpec>,
    pub deploy: Option<DeployStrategy>,
    pub placement: Option<PlacementConstraint>,
    /// Wasm triggers: "http:/path", "cron:expr", "queue:topic", "event:pattern"
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Static assets directory (for builtin:static-server Wasm module).
    pub assets: Option<String>,
}
