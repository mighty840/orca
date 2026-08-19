//! Helper to build Docker container configs from a [`WorkloadSpec`].

mod helpers;

use std::collections::HashMap;

use bollard::container::Config;
use bollard::models::{HostConfig, PortBinding, RestartPolicy, RestartPolicyNameEnum};

use orca_core::types::WorkloadSpec;

use helpers::{
    build_all_binds, build_gpu_passthrough, build_labels, build_log_config, build_port_config,
    parse_extra_port, parse_resource_limits,
};

/// Build a Docker container [`Config`] from a workload spec.
pub(crate) fn build_container_config(spec: &WorkloadSpec) -> Config<String> {
    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    let (mut port_bindings, mut exposed_ports) = build_port_config(spec.port, spec.host_port);
    // `extra_ports` accepts the docker-compose forms:
    //   `host:container`               (defaults to 0.0.0.0 + tcp)
    //   `host:container/proto`         (e.g. `10000:10000/udp` for Jitsi JVB media)
    //   `host_ip:host:container[/proto]` (bind to a specific host address)
    for entry in &spec.extra_ports {
        let Some(parsed) = parse_extra_port(entry) else {
            continue;
        };
        let key = format!("{}/{}", parsed.container_port, parsed.proto);
        exposed_ports.insert(key.clone(), HashMap::new());
        port_bindings.insert(
            key,
            Some(vec![PortBinding {
                host_ip: Some(parsed.host_ip),
                host_port: Some(parsed.host_port),
            }]),
        );
    }
    let binds = build_all_binds(spec);
    let gpu = build_gpu_passthrough(spec);
    let labels = build_labels(spec);

    let (memory_limit, nano_cpus) = parse_resource_limits(spec);

    let log_config = build_log_config();

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        binds: if binds.is_empty() { None } else { Some(binds) },
        device_requests: if gpu.device_requests.is_empty() {
            None
        } else {
            Some(gpu.device_requests)
        },
        devices: if gpu.devices.is_empty() {
            None
        } else {
            Some(gpu.devices)
        },
        group_add: if gpu.group_add.is_empty() {
            None
        } else {
            Some(gpu.group_add)
        },
        memory: memory_limit,
        nano_cpus,
        log_config: Some(log_config),
        restart_policy: build_restart_policy(spec.restart_policy.as_deref()),
        ..Default::default()
    };

    Config {
        image: Some(spec.image.clone()),
        env: if env.is_empty() { None } else { Some(env) },
        exposed_ports: if exposed_ports.is_empty() {
            None
        } else {
            Some(exposed_ports)
        },
        cmd: if spec.cmd.is_empty() {
            None
        } else {
            Some(spec.cmd.clone())
        },
        host_config: Some(host_config),
        labels: Some(labels),
        ..Default::default()
    }
}

/// Derive the Docker network name for a service.
pub(crate) fn network_name(spec: &WorkloadSpec) -> String {
    if let Some(net) = &spec.network {
        format!("orca-{net}")
    } else {
        // Derive from service name prefix (e.g., "kitchenasty-db" → "orca-kitchenasty")
        let prefix = spec.name.split('-').next().unwrap_or(&spec.name);
        format!("orca-{prefix}")
    }
}

/// Detect a network-identity change for a service being (re)created (#163).
///
/// Given the network the service is about to be created on (`derived`) and the
/// orca networks its existing container is currently attached to (`attached`),
/// return the existing service network if it differs from `derived`. A
/// mismatch means the redeploy would silently move the service off the network
/// its project siblings share — their connections black-hole (aliases stop
/// resolving) while the deploy still reports success.
///
/// `orca-internal` (the shared cross-service network that every `internal`
/// service also joins) and non-orca networks (`bridge`, etc.) are ignored:
/// only a real per-service orca network counts as drift.
pub(crate) fn drifted_service_network(derived: &str, attached: &[String]) -> Option<String> {
    attached
        .iter()
        .find(|n| n.starts_with("orca-") && n.as_str() != "orca-internal" && n.as_str() != derived)
        .cloned()
}

/// Map the spec's restart-policy string to Docker's (#121). Invalid values
/// are rejected at config load (`ServiceConfig::validate`); anything
/// unrecognized here degrades to None (Docker default: no restart) rather
/// than failing the deploy.
fn build_restart_policy(policy: Option<&str>) -> Option<RestartPolicy> {
    let policy = policy?;
    let (name, retries) = match policy {
        "no" => (RestartPolicyNameEnum::NO, None),
        "always" => (RestartPolicyNameEnum::ALWAYS, None),
        "unless-stopped" => (RestartPolicyNameEnum::UNLESS_STOPPED, None),
        _ => match policy.strip_prefix("on-failure") {
            Some("") => (RestartPolicyNameEnum::ON_FAILURE, None),
            Some(rest) => (
                RestartPolicyNameEnum::ON_FAILURE,
                rest.strip_prefix(':').and_then(|n| n.parse::<i64>().ok()),
            ),
            None => return None,
        },
    };
    Some(RestartPolicy {
        name: Some(name),
        maximum_retry_count: retries,
    })
}

#[cfg(test)]
mod restart_policy_tests {
    use super::*;

    #[test]
    fn restart_policy_mapping() {
        use bollard::models::RestartPolicyNameEnum as N;
        assert!(build_restart_policy(None).is_none());
        assert_eq!(
            build_restart_policy(Some("always")).unwrap().name,
            Some(N::ALWAYS)
        );
        assert_eq!(
            build_restart_policy(Some("unless-stopped")).unwrap().name,
            Some(N::UNLESS_STOPPED)
        );
        let p = build_restart_policy(Some("on-failure:5")).unwrap();
        assert_eq!(p.name, Some(N::ON_FAILURE));
        assert_eq!(p.maximum_retry_count, Some(5));
        let p = build_restart_policy(Some("on-failure")).unwrap();
        assert_eq!(p.maximum_retry_count, None);
        assert!(build_restart_policy(Some("garbage")).is_none());
    }
}

#[cfg(test)]
mod network_drift_tests {
    use super::drifted_service_network;

    #[test]
    fn no_drift_when_on_the_derived_network() {
        let attached = vec!["orca-dxseo".to_string(), "orca-internal".to_string()];
        assert_eq!(drifted_service_network("orca-dxseo", &attached), None);
    }

    #[test]
    fn drift_when_existing_service_network_differs() {
        // The #163 case: existing container on orca-dxseo, redeploy derives
        // orca-dx-seo — the service would move and split from its siblings.
        let attached = vec!["orca-dxseo".to_string()];
        assert_eq!(
            drifted_service_network("orca-dx-seo", &attached),
            Some("orca-dxseo".to_string())
        );
    }

    #[test]
    fn internal_and_non_orca_networks_are_ignored() {
        let attached = vec!["orca-internal".to_string(), "bridge".to_string()];
        assert_eq!(drifted_service_network("orca-app", &attached), None);
    }

    #[test]
    fn no_container_no_drift() {
        assert_eq!(drifted_service_network("orca-app", &[]), None);
    }
}
