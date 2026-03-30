//! Routing table management for container and Wasm workloads.

use tracing::info;

use orca_core::config::ServiceConfig;
use orca_core::types::{WorkloadSpec, WorkloadStatus};

use crate::state::{AppState, WasmTrigger};
use orca_proxy::RouteTarget;

/// Update the container routing table for a service.
pub(crate) async fn update_container_routes(state: &AppState, config: &ServiceConfig) {
    let Some(domain) = &config.domain else {
        return;
    };

    let services = state.services.read().await;
    let Some(svc) = services.get(&config.name) else {
        return;
    };

    let targets: Vec<RouteTarget> = svc
        .instances
        .iter()
        .filter(|i| i.status == WorkloadStatus::Running)
        .filter_map(|i| {
            i.host_port.map(|port| RouteTarget {
                address: format!("127.0.0.1:{port}"),
                service_name: config.name.clone(),
            })
        })
        .collect();

    drop(services);

    let mut route_table = state.route_table.write().await;
    if targets.is_empty() {
        route_table.remove(domain);
    } else {
        route_table.insert(domain.clone(), targets);
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
pub(crate) fn service_config_to_spec(config: &ServiceConfig) -> anyhow::Result<WorkloadSpec> {
    let image = config
        .image
        .clone()
        .or_else(|| config.module.clone())
        .ok_or_else(|| anyhow::anyhow!("service '{}' has no image or module", config.name))?;

    Ok(WorkloadSpec {
        name: config.name.clone(),
        runtime: config.runtime,
        image,
        replicas: config.replicas.clone(),
        port: config.port,
        domain: config.domain.clone(),
        health: config.health.clone(),
        env: config.env.clone(),
        resources: config.resources.clone(),
        volume: config.volume.clone(),
        deploy: config.deploy.clone(),
        placement: config.placement.clone(),
        triggers: config
            .triggers
            .iter()
            .filter_map(|t| t.clone().try_into().ok())
            .collect(),
    })
}
