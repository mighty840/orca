//! Routing table management for container and Wasm workloads.

use std::collections::HashMap;

use tracing::info;

use orca_core::config::ServiceConfig;
use orca_core::types::{DeployKind, HealthState, WorkloadSpec, WorkloadStatus};

/// Resolve `${secrets.KEY}` patterns in env vars using the configured
/// secrets store. Project-scoped keys (`<project>.KEY`, #68) win over
/// bare global keys for services that belong to a project.
fn resolve_secrets(
    env: &HashMap<String, String>,
    project: Option<&str>,
) -> HashMap<String, String> {
    match orca_core::secrets::open_configured() {
        Ok(store) => store.resolve_env_scoped(env, project),
        Err(_) => env.clone(),
    }
}

/// Derive the Docker network name for a workload spec.
pub(crate) fn service_network_name(spec: &WorkloadSpec) -> String {
    if let Some(net) = &spec.network {
        format!("orca-{net}")
    } else {
        let prefix = spec.name.split('-').next().unwrap_or(&spec.name);
        format!("orca-{prefix}")
    }
}

use crate::state::{AppState, WasmTrigger};
use orca_proxy::RouteTarget;

/// Update the container routing table for a service.
///
/// Filters for healthy/no-check instances only. Called during deploy and
/// periodically by the watchdog to clean up stale routes.
pub async fn update_container_routes(state: &AppState, config: &ServiceConfig) {
    let domains = config.all_domains();
    if domains.is_empty() {
        return;
    }

    let services = state.services.read().await;
    let Some(svc) = services.get(&config.name) else {
        return;
    };

    // Build route path pattern from config
    let path_pattern = config.routes.first().cloned();

    // Determine canary weight split
    let is_canary_deploy = config
        .deploy
        .as_ref()
        .is_some_and(|d| d.strategy == DeployKind::Canary);
    let canary_weight = config
        .deploy
        .as_ref()
        .map(|d| d.canary_weight)
        .unwrap_or(20);
    let stable_weight = 100u32.saturating_sub(canary_weight);

    let targets: Vec<RouteTarget> = svc
        .instances
        .iter()
        .filter(|i| i.status == WorkloadStatus::Running)
        .filter(|i| matches!(i.health, HealthState::Healthy | HealthState::NoCheck))
        .filter_map(|i| {
            let address = i
                .host_port
                .map(|port| format!("127.0.0.1:{port}"))
                .or_else(|| i.container_address.clone());
            let weight = if is_canary_deploy {
                if i.is_canary {
                    canary_weight
                } else {
                    stable_weight
                }
            } else {
                100
            };
            address.map(|addr| RouteTarget {
                address: addr,
                service_name: config.name.clone(),
                path_pattern: path_pattern.clone(),
                strip_prefix: config.strip_prefix.clone(),
                weight,
            })
        })
        .collect();

    drop(services);

    // Each domain routes to the same set of targets (same container(s)).
    //
    // Merge, don't replace: several services can share one domain with
    // disjoint `routes` patterns (e.g. storefront on /* and admin on
    // /admin/* of the same host). A plain insert() would make the last
    // service to register wipe its siblings' targets — observed as
    // whole-path-trees 404ing after every deploy/health transition. Only
    // this service's own stale targets are dropped before extending; the
    // removal paths (remove/pause) already use the same retain semantics.
    let mut route_table = state.route_table.write().await;
    for domain in &domains {
        let entry = route_table.entry(domain.clone()).or_default();
        entry.retain(|t| t.service_name != config.name);
        entry.extend(targets.iter().cloned());
        if entry.is_empty() {
            route_table.remove(domain);
        }
    }
}

/// Update the Wasm trigger table for a service.
pub(crate) async fn update_wasm_triggers(state: &AppState, config: &ServiceConfig) {
    let services = state.services.read().await;
    let Some(svc) = services.get(&config.name) else {
        return;
    };

    let runtime_id = svc
        .instances
        .iter()
        .find(|i| i.status == WorkloadStatus::Running)
        .map(|i| i.handle.runtime_id.clone());

    drop(services);

    let Some(runtime_id) = runtime_id else {
        return;
    };

    let mut triggers = state.wasm_triggers.write().await;

    // Remove existing triggers for this service
    triggers.retain(|t| t.service_name != config.name);

    // Add triggers for each HTTP trigger pattern
    for trigger_str in &config.triggers {
        if let Some(path) = trigger_str.strip_prefix("http:") {
            triggers.push(WasmTrigger {
                pattern: path.to_string(),
                runtime_id: runtime_id.clone(),
                service_name: config.name.clone(),
            });
            info!("Registered Wasm trigger: {} -> {}", path, config.name);
        }
    }
}

/// Convert a [`ServiceConfig`] into a [`WorkloadSpec`] for the runtime.
///
/// When `build` is configured, the image field uses a placeholder that the
/// reconciler replaces after building. If neither `image`, `module`, nor `build`
/// is set, an error is returned.
pub(crate) fn service_config_to_spec(config: &ServiceConfig) -> anyhow::Result<WorkloadSpec> {
    let image = config
        .image
        .clone()
        .or_else(|| config.module.clone())
        .or_else(|| {
            config
                .build
                .as_ref()
                .map(|_| format!("orca-build-{}:pending", config.name))
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "service '{}' has no image, module, or build config",
                config.name
            )
        })?;

    Ok(WorkloadSpec {
        name: config.name.clone(),
        runtime: config.runtime,
        image,
        replicas: config.replicas.clone(),
        port: config.port,
        host_port: config.host_port,
        domain: config.primary_domain(),
        domains: config.all_domains(),
        routes: config.routes.clone(),
        health: config.health.clone(),
        readiness: config.readiness.clone(),
        liveness: config.liveness.clone(),
        env: resolve_secrets(&config.env, config.project.as_deref()),
        resources: config.resources.clone(),
        volume: config.volume.clone(),
        deploy: config.deploy.clone(),
        placement: config.placement.clone(),
        network: config.network.clone(),
        aliases: config.aliases.clone(),
        mounts: config.mounts.clone(),
        triggers: config
            .triggers
            .iter()
            .filter_map(|t| t.clone().try_into().ok())
            .collect(),
        build: config.build.clone(),
        tls_cert: config.tls_cert.clone(),
        tls_key: config.tls_key.clone(),
        internal: config.internal,
        cmd: config.cmd.clone(),
        extra_ports: config.extra_ports.clone(),
        strip_prefix: config.strip_prefix.clone(),
        pull_policy: config.pull_policy,
    })
}

#[cfg(test)]
#[path = "routes_tests_spec.rs"]
mod tests_spec;

#[cfg(test)]
#[path = "routes_tests_health.rs"]
mod tests_health;

#[cfg(test)]
#[path = "routes_tests_merge.rs"]
mod tests_merge;
