//! Helper to build Docker container configs from a [`WorkloadSpec`].

mod helpers;

use std::collections::HashMap;

use bollard::container::Config;
use bollard::models::{HostConfig, PortBinding};

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
