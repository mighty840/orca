//! Helper to build Docker container configs from a [`WorkloadSpec`].

use std::collections::HashMap;

use bollard::container::Config;
use bollard::models::{HostConfig, HostConfigLogConfig, PortBinding};

use super::ORCA_LABEL;
use orca_core::types::WorkloadSpec;

/// Build a Docker container [`Config`] from a workload spec.
pub(crate) fn build_container_config(spec: &WorkloadSpec) -> Config<String> {
    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    let (port_bindings, exposed_ports) = build_port_config(spec.port, spec.host_port);
    let binds = build_all_binds(spec);
    let device_requests = build_gpu_requests(spec);
    let labels = build_labels(spec);

    let (memory_limit, nano_cpus) = parse_resource_limits(spec);

    let log_config = build_log_config();

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        binds: if binds.is_empty() { None } else { Some(binds) },
        device_requests: if device_requests.is_empty() {
            None
        } else {
            Some(device_requests)
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

type PortBindings = HashMap<String, Option<Vec<PortBinding>>>;
type ExposedPorts = HashMap<String, HashMap<(), ()>>;

fn build_port_config(port: Option<u16>, host_port: Option<u16>) -> (PortBindings, ExposedPorts) {
    let mut port_bindings = HashMap::new();
    let mut exposed_ports = HashMap::new();
    if let Some(port) = port {
        let key = format!("{port}/tcp");
        exposed_ports.insert(key.clone(), HashMap::new());
        let hp = host_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "0".to_string());
        port_bindings.insert(
            key,
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(hp),
            }]),
        );
    }
    (port_bindings, exposed_ports)
}

fn build_all_binds(spec: &WorkloadSpec) -> Vec<String> {
    let mut binds = Vec::new();
    // Named volume
    if let Some(vol) = &spec.volume {
        let vol_name = format!("orca-{}-data", spec.name);
        binds.push(format!("{vol_name}:{}", vol.path));
    }
    // Host bind mounts
    for mount in &spec.mounts {
        binds.push(mount.clone());
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

/// Parse resource limits from the workload spec into Docker host config values.
fn parse_resource_limits(spec: &WorkloadSpec) -> (Option<i64>, Option<i64>) {
    let res = match &spec.resources {
        Some(r) => r,
        None => return (None, None),
    };
    let memory = res.memory.as_deref().and_then(parse_memory_string);
    let nano_cpus = res.cpu.map(|c| (c * 1e9) as i64);
    (memory, nano_cpus)
}

/// Parse a human-readable memory string (e.g. "512Mi", "2Gi") into bytes.
fn parse_memory_string(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(val) = s.strip_suffix("Gi") {
        val.parse::<u64>()
            .ok()
            .map(|v| (v * 1024 * 1024 * 1024) as i64)
    } else if let Some(val) = s.strip_suffix("Mi") {
        val.parse::<u64>().ok().map(|v| (v * 1024 * 1024) as i64)
    } else if let Some(val) = s.strip_suffix("Ki") {
        val.parse::<u64>().ok().map(|v| (v * 1024) as i64)
    } else {
        s.parse::<i64>().ok()
    }
}

fn build_log_config() -> HostConfigLogConfig {
    let mut config = HashMap::new();
    config.insert("max-size".to_string(), "10m".to_string());
    config.insert("max-file".to_string(), "3".to_string());
    HostConfigLogConfig {
        typ: Some("json-file".to_string()),
        config: Some(config),
    }
}

fn build_labels(spec: &WorkloadSpec) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    labels.insert(ORCA_LABEL.to_string(), "true".to_string());
    labels.insert("orca.service".to_string(), spec.name.clone());
    if let Some(net) = &spec.network {
        labels.insert("orca.network".to_string(), net.clone());
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::types::{Replicas, ResourceLimits, RuntimeKind};

    fn minimal_spec() -> WorkloadSpec {
        WorkloadSpec {
            name: "test".to_string(),
            runtime: RuntimeKind::Container,
            image: "nginx:latest".to_string(),
            replicas: Replicas::Fixed(1),
            port: None,
            host_port: None,
            domain: None,
            routes: vec![],
            health: None,
            readiness: None,
            liveness: None,
            env: Default::default(),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: None,
            aliases: vec![],
            mounts: vec![],
            triggers: vec![],
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
        }
    }

    #[test]
    fn test_parse_memory_ki() {
        assert_eq!(parse_memory_string("64Ki"), Some(65536));
    }

    #[test]
    fn test_parse_memory_mi() {
        assert_eq!(parse_memory_string("512Mi"), Some(536870912));
    }

    #[test]
    fn test_parse_memory_gi() {
        assert_eq!(parse_memory_string("2Gi"), Some(2147483648));
    }

    #[test]
    fn test_parse_memory_plain_bytes() {
        assert_eq!(parse_memory_string("1048576"), Some(1048576));
    }

    #[test]
    fn test_parse_memory_invalid() {
        assert_eq!(parse_memory_string("abc"), None);
    }

    #[test]
    fn test_resource_limits_sets_nano_cpus() {
        let mut spec = minimal_spec();
        spec.resources = Some(ResourceLimits {
            memory: None,
            cpu: Some(2.0),
            gpu: None,
        });
        let (_mem, nano_cpus) = parse_resource_limits(&spec);
        assert_eq!(nano_cpus, Some(2_000_000_000));
    }

    #[test]
    fn test_log_config() {
        let cfg = build_log_config();
        assert_eq!(cfg.typ.as_deref(), Some("json-file"));
        let opts = cfg.config.as_ref().unwrap();
        assert_eq!(opts.get("max-size").map(|s| s.as_str()), Some("10m"));
        assert_eq!(opts.get("max-file").map(|s| s.as_str()), Some("3"));
    }

    #[test]
    fn test_memory_limit_zero() {
        // "0" should parse as 0 bytes (no effective limit).
        assert_eq!(parse_memory_string("0"), Some(0));
    }

    #[test]
    fn test_cpu_limit_zero() {
        let mut spec = minimal_spec();
        spec.resources = Some(ResourceLimits {
            memory: None,
            cpu: Some(0.0),
            gpu: None,
        });
        let (_mem, nano_cpus) = parse_resource_limits(&spec);
        assert_eq!(nano_cpus, Some(0), "cpu=0.0 should produce 0 nano_cpus");
    }

    #[test]
    fn test_very_large_memory() {
        // 128Gi = 128 * 1024^3 = 137438953472
        assert_eq!(parse_memory_string("128Gi"), Some(137_438_953_472));
    }
}
