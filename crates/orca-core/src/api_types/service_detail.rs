//! `GET /api/v1/services/{name}` and `.../history` (#80, shared with #259 and
//! #265): one service's status and configuration, safe to show to a viewer.
//!
//! The configuration is an allowlist, not `ServiceConfig` with the env
//! patched: `cmd`, the backup pre-hook, build settings and TLS paths can carry
//! secrets too, so they are left out or reduced to a flag. Env keys are
//! shown; a value is shown only when it is a single `${secrets...}`
//! reference, which names a secret without revealing it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ServiceStatus;
use crate::config::{ProbeConfig, ServiceConfig};
use crate::types::RuntimeKind;
use crate::types::{DeployStrategy, PlacementConstraint, Replicas, ResourceLimits, VolumeSpec};

/// What a masked env value reads as.
pub const MASKED: &str = "***";

/// Response from `GET /api/v1/services/{name}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDetail {
    /// The same entry `GET /api/v1/status` lists for this service.
    pub status: ServiceStatus,
    pub config: ServiceConfigView,
}

/// One entry of `GET /api/v1/services/{name}/history`, newest first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRecordView {
    pub deploy_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub config: ServiceConfigView,
}

/// A service's configuration without anything that can carry a secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfigView {
    pub name: String,
    #[serde(default)]
    pub project: Option<String>,
    pub runtime: RuntimeKind,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    pub replicas: Replicas,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub host_port: Option<u16>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub readiness: Option<ProbeConfig>,
    #[serde(default)]
    pub liveness: Option<ProbeConfig>,
    /// Keys with masked values; see the module docs.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Every secret the env references (`KEY` or `scope.KEY`), sorted.
    #[serde(default)]
    pub secret_refs: Vec<String>,
    #[serde(default)]
    pub resources: Option<ResourceLimits>,
    #[serde(default)]
    pub volume: Option<VolumeSpec>,
    #[serde(default)]
    pub deploy: Option<DeployStrategy>,
    #[serde(default)]
    pub placement: Option<PlacementConstraint>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<String>,
    #[serde(default)]
    pub extra_ports: Vec<String>,
    #[serde(default)]
    pub internal: bool,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub strip_prefix: Option<String>,
    #[serde(default)]
    pub restart_policy: Option<String>,
    /// Built from source rather than pulled.
    #[serde(default)]
    pub build: bool,
    /// Has a command override (not shown: it can carry credentials).
    #[serde(default)]
    pub cmd: bool,
    /// Has a backup pre-hook (not shown, for the same reason).
    #[serde(default)]
    pub backup_hook: bool,
    /// Registered by orphan adoption, not deployed from file (#313).
    #[serde(default)]
    pub adopted: bool,
}

impl From<&ServiceConfig> for ServiceConfigView {
    fn from(c: &ServiceConfig) -> Self {
        let mut refs: Vec<String> = c
            .env
            .values()
            .flat_map(|v| crate::secrets::extract_refs(v))
            .map(|r| match r.scope {
                Some(scope) => format!("{scope}.{}", r.key),
                None => r.key,
            })
            .collect();
        refs.sort();
        refs.dedup();
        Self {
            name: c.name.clone(),
            project: c.project.clone(),
            runtime: c.runtime,
            image: c.image.clone(),
            module: c.module.clone(),
            replicas: c.replicas.clone(),
            port: c.port,
            host_port: c.host_port,
            domains: c.all_domains(),
            routes: c.routes.clone(),
            health: c.health.clone(),
            readiness: c.readiness.clone(),
            liveness: c.liveness.clone(),
            env: c
                .env
                .iter()
                .map(|(k, v)| (k.clone(), mask_value(v)))
                .collect(),
            secret_refs: refs,
            resources: c.resources.clone(),
            volume: c.volume.clone(),
            deploy: c.deploy.clone(),
            placement: c.placement.clone(),
            network: c.network.clone(),
            aliases: c.aliases.clone(),
            mounts: c.mounts.clone(),
            extra_ports: c.extra_ports.clone(),
            internal: c.internal,
            depends_on: c.depends_on.clone(),
            strip_prefix: c.strip_prefix.clone(),
            restart_policy: c.restart_policy.clone(),
            build: c.build.is_some(),
            cmd: !c.cmd.is_empty(),
            backup_hook: c.backup.as_ref().is_some_and(|b| b.pre_hook.is_some()),
            adopted: c.adopted,
        }
    }
}

/// Keep a value that is exactly one `${secrets...}` reference, or empty;
/// mask anything else, including a reference embedded in literal text
/// (`postgres://user:pw@${secrets.HOST}`).
fn mask_value(v: &str) -> String {
    let refs = crate::secrets::extract_refs(v);
    let single_ref = match refs.as_slice() {
        [r] if !r.key.is_empty() => {
            let scope = r
                .scope
                .as_deref()
                .map(|s| format!("{s}."))
                .unwrap_or_default();
            v == format!("${{secrets.{scope}{}}}", r.key)
        }
        _ => false,
    };
    if v.is_empty() || single_ref {
        v.to_string()
    } else {
        MASKED.to_string()
    }
}

#[cfg(test)]
#[path = "service_detail_tests.rs"]
mod tests;
