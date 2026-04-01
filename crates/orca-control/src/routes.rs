//! Routing table management for container and Wasm workloads.

use tracing::info;

use orca_core::config::ServiceConfig;
use orca_core::types::{HealthState, WorkloadSpec, WorkloadStatus};

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
pub(crate) async fn update_container_routes(state: &AppState, config: &ServiceConfig) {
    let Some(domain) = &config.domain else {
        return;
    };

    let services = state.services.read().await;
    let Some(svc) = services.get(&config.name) else {
        return;
    };

    // Build route path pattern from config
    let path_pattern = config.routes.first().cloned();

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
            address.map(|addr| RouteTarget {
                address: addr,
                service_name: config.name.clone(),
                path_pattern: path_pattern.clone(),
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
        host_port: config.host_port,
        domain: config.domain.clone(),
        routes: config.routes.clone(),
        health: config.health.clone(),
        readiness: config.readiness.clone(),
        liveness: config.liveness.clone(),
        env: config.env.clone(),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use orca_core::config::ServiceConfig;
    use orca_core::types::Replicas;

    fn minimal_config(image: Option<String>, module: Option<String>) -> ServiceConfig {
        ServiceConfig {
            name: "test-svc".to_string(),
            runtime: Default::default(),
            image,
            module,
            replicas: Replicas::Fixed(1),
            port: Some(8080),
            domain: Some("test.example.com".to_string()),
            health: None,
            readiness: None,
            liveness: None,
            env: HashMap::new(),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: None,
            aliases: vec![],
            mounts: vec![],
            routes: vec![],
            host_port: None,
            triggers: Vec::new(),
            assets: None,
        }
    }

    #[test]
    fn config_to_spec_with_image() {
        let config = minimal_config(Some("nginx:latest".to_string()), None);
        let spec = service_config_to_spec(&config).unwrap();
        assert_eq!(spec.name, "test-svc");
        assert_eq!(spec.image, "nginx:latest");
        assert_eq!(spec.port, Some(8080));
        assert_eq!(spec.domain.as_deref(), Some("test.example.com"));
    }

    #[test]
    fn config_to_spec_errors_without_image_or_module() {
        let config = minimal_config(None, None);
        let result = service_config_to_spec(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("no image or module"),
            "unexpected error: {err_msg}"
        );
    }

    use crate::state::InstanceState;
    use orca_core::runtime::WorkloadHandle;

    fn make_instance(health: HealthState, port: Option<u16>) -> InstanceState {
        InstanceState {
            handle: WorkloadHandle {
                runtime_id: "r".into(),
                name: "n".into(),
                metadata: HashMap::new(),
            },
            status: WorkloadStatus::Running,
            host_port: port,
            container_address: None,
            health,
        }
    }

    /// Only Healthy and NoCheck instances should be routable.
    #[test]
    fn health_filter_includes_healthy_and_nocheck() {
        let instances = vec![
            make_instance(HealthState::Healthy, Some(8080)),
            make_instance(HealthState::NoCheck, Some(8081)),
            make_instance(HealthState::Unhealthy, Some(8082)),
            make_instance(HealthState::Unknown, Some(8083)),
        ];
        let routable: Vec<_> = instances
            .iter()
            .filter(|i| i.status == WorkloadStatus::Running)
            .filter(|i| matches!(i.health, HealthState::Healthy | HealthState::NoCheck))
            .collect();
        assert_eq!(routable.len(), 2);
        assert_eq!(routable[0].host_port, Some(8080));
        assert_eq!(routable[1].host_port, Some(8081));
    }

    /// All-unhealthy instances should produce an empty route set.
    #[test]
    fn health_filter_excludes_all_unhealthy() {
        let instances = vec![
            make_instance(HealthState::Unhealthy, Some(8080)),
            make_instance(HealthState::Unknown, Some(8081)),
        ];
        let routable: Vec<_> = instances
            .iter()
            .filter(|i| i.status == WorkloadStatus::Running)
            .filter(|i| matches!(i.health, HealthState::Healthy | HealthState::NoCheck))
            .collect();
        assert!(routable.is_empty());
    }
}
