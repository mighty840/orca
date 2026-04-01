//! Routing table management for container and Wasm workloads.

use std::collections::HashMap;

use tracing::info;

use orca_core::config::ServiceConfig;
use orca_core::types::{HealthState, WorkloadSpec, WorkloadStatus};

/// Resolve `${secrets.KEY}` patterns in env vars using the local secrets store.
fn resolve_secrets(env: &HashMap<String, String>) -> HashMap<String, String> {
    match orca_core::secrets::SecretStore::open("secrets.json") {
        Ok(store) => store.resolve_env(env),
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
        domain: config.domain.clone(),
        routes: config.routes.clone(),
        health: config.health.clone(),
        readiness: config.readiness.clone(),
        liveness: config.liveness.clone(),
        env: resolve_secrets(&config.env),
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
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
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
            err_msg.contains("no image, module, or build config"),
            "unexpected error: {err_msg}"
        );
    }

    #[test]
    fn config_to_spec_with_build_config() {
        let mut config = minimal_config(None, None);
        config.build = Some(orca_core::config::BuildConfig {
            repo: "git@github.com:org/repo.git".to_string(),
            branch: None,
            dockerfile: None,
            context: None,
        });
        let spec = service_config_to_spec(&config).unwrap();
        assert!(spec.image.starts_with("orca-build-test-svc:"));
        assert!(spec.build.is_some());
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

    /// Secret patterns in env vars must be resolved by service_config_to_spec.
    #[test]
    fn config_to_spec_resolves_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let secrets_path = dir.path().join("secrets.json");

        // Use the default master key so resolve_secrets() (which calls open())
        // can decrypt with the same key.
        let mut store = orca_core::secrets::SecretStore::open(&secrets_path).unwrap();
        store.set("DB_PASS", "hunter2").unwrap();
        drop(store);

        // Copy secrets.json to "secrets.json" relative (where resolve_secrets looks)
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let mut config = minimal_config(Some("postgres:16".into()), None);
        config
            .env
            .insert("POSTGRES_PASSWORD".into(), "${secrets.DB_PASS}".into());
        config.env.insert("PLAIN".into(), "unchanged".into());

        let spec = service_config_to_spec(&config).unwrap();
        assert_eq!(spec.env["POSTGRES_PASSWORD"], "hunter2");
        assert_eq!(spec.env["PLAIN"], "unchanged");

        std::env::set_current_dir(original_dir).unwrap();
    }

    /// When no secrets file exists, env vars pass through unchanged.
    #[test]
    fn config_to_spec_no_secrets_file_passes_env_through() {
        let dir = tempfile::tempdir().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let mut config = minimal_config(Some("nginx:latest".into()), None);
        config
            .env
            .insert("SECRET_VAR".into(), "${secrets.MISSING}".into());

        let spec = service_config_to_spec(&config).unwrap();
        // No secrets file -> env returned as-is (clone fallback)
        assert_eq!(spec.env["SECRET_VAR"], "${secrets.MISSING}");

        std::env::set_current_dir(original_dir).unwrap();
    }
}
