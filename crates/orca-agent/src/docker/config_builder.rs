//! Helper to build Docker container configs from a [`WorkloadSpec`].

use std::collections::HashMap;

use bollard::container::Config;
use bollard::models::{HostConfig, PortBinding};

use super::ORCA_LABEL;
use orca_core::types::WorkloadSpec;

/// Build a Docker container [`Config`] from a workload spec.
pub(crate) fn build_container_config(spec: &WorkloadSpec) -> Config<String> {
    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    let (port_bindings, exposed_ports) = build_port_config(spec.port);
    let binds = build_volume_binds(spec);
    let device_requests = build_gpu_requests(spec);
    let labels = build_labels(spec);

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        binds: if binds.is_empty() { None } else { Some(binds) },
        device_requests: if device_requests.is_empty() {
            None
        } else {
            Some(device_requests)
        },
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
        host_config: Some(host_config),
        labels: Some(labels),
        ..Default::default()
    }
}

type PortBindings = HashMap<String, Option<Vec<PortBinding>>>;
type ExposedPorts = HashMap<String, HashMap<(), ()>>;

fn build_port_config(port: Option<u16>) -> (PortBindings, ExposedPorts) {
    let mut port_bindings = HashMap::new();
    let mut exposed_ports = HashMap::new();
    if let Some(port) = port {
        let key = format!("{port}/tcp");
        exposed_ports.insert(key.clone(), HashMap::new());
        port_bindings.insert(
            key,
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some("0".to_string()),
            }]),
        );
    }
    (port_bindings, exposed_ports)
}

fn build_volume_binds(spec: &WorkloadSpec) -> Vec<String> {
    let mut binds = Vec::new();
    if let Some(vol) = &spec.volume {
        let vol_name = format!("orca-{}-data", spec.name);
        binds.push(format!("{vol_name}:{}", vol.path));
    }
    binds
}

fn build_gpu_requests(spec: &WorkloadSpec) -> Vec<bollard::models::DeviceRequest> {
    let mut device_requests = Vec::new();
    if let Some(res) = &spec.resources
        && let Some(gpu) = &res.gpu
    {
        device_requests.push(bollard::models::DeviceRequest {
            count: Some(gpu.count as i64),
            driver: Some("nvidia".to_string()),
            capabilities: Some(vec![vec!["gpu".to_string()]]),
            ..Default::default()
        });
    }
    device_requests
}

fn build_labels(spec: &WorkloadSpec) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    labels.insert(ORCA_LABEL.to_string(), "true".to_string());
    labels.insert("orca.service".to_string(), spec.name.clone());
    labels
}
