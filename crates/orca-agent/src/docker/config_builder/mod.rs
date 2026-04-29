//! Helper to build Docker container configs from a [`WorkloadSpec`].

mod helpers;

use std::collections::HashMap;

use bollard::container::Config;
use bollard::models::{HostConfig, PortBinding};

use orca_core::types::WorkloadSpec;

use helpers::{
    build_all_binds, build_gpu_passthrough, build_labels, build_log_config, build_port_config,
    parse_resource_limits,
};

/// Build a Docker container [`Config`] from a workload spec.
pub(crate) fn build_container_config(spec: &WorkloadSpec) -> Config<String> {
    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    let (mut port_bindings, mut exposed_ports) = build_port_config(spec.port, spec.host_port);
    // `extra_ports` accepts both `host:container` (defaults to tcp) and
    // `host:container/proto`, e.g. `10000:10000/udp` for Jitsi JVB media.
    for entry in &spec.extra_ports {
        let Some((host, rest)) = entry.split_once(':') else {
            continue;
        };
        let (container, proto) = match rest.rsplit_once('/') {
            Some((c, p)) => (c, p),
            None => (rest, "tcp"),
        };
        let key = format!("{container}/{proto}");
        exposed_ports.insert(key.clone(), HashMap::new());
        port_bindings.insert(
            key,
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(host.to_string()),
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
