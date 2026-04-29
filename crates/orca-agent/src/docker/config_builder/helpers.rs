//! Helper functions for building Docker container configs: ports, mounts,
//! GPU passthrough, resource limits, log config, and labels.

use std::collections::HashMap;

use bollard::models::{HostConfigLogConfig, PortBinding};

use crate::docker::ORCA_LABEL;
use orca_core::types::WorkloadSpec;

pub(super) type PortBindings = HashMap<String, Option<Vec<PortBinding>>>;
pub(super) type ExposedPorts = HashMap<String, HashMap<(), ()>>;

pub(super) fn build_port_config(
    port: Option<u16>,
    host_port: Option<u16>,
) -> (PortBindings, ExposedPorts) {
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

pub(super) fn build_all_binds(spec: &WorkloadSpec) -> Vec<String> {
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

/// GPU passthrough configuration for Docker containers.
///
/// - **nvidia**: Uses Docker's `--gpus` via DeviceRequest (requires nvidia-container-toolkit).
/// - **amd**: Mounts `/dev/kfd` + `/dev/dri` devices directly (requires ROCm on host).
/// - **unspecified vendor**: Defaults to nvidia DeviceRequest.
pub(super) struct GpuPassthrough {
    pub(super) device_requests: Vec<bollard::models::DeviceRequest>,
    pub(super) devices: Vec<bollard::models::DeviceMapping>,
    pub(super) group_add: Vec<String>,
}

pub(super) fn build_gpu_passthrough(spec: &WorkloadSpec) -> GpuPassthrough {
    let mut result = GpuPassthrough {
        device_requests: Vec::new(),
        devices: Vec::new(),
        group_add: Vec::new(),
    };

    let Some(res) = &spec.resources else {
        return result;
    };
    let Some(gpu) = &res.gpu else {
        return result;
    };

    let vendor = gpu.vendor.as_deref().unwrap_or("nvidia");
    match vendor {
        "amd" | "rocm" => {
            // AMD ROCm: mount /dev/kfd (kernel fusion driver) + /dev/dri (render nodes)
            result.devices.push(bollard::models::DeviceMapping {
                path_on_host: Some("/dev/kfd".into()),
                path_in_container: Some("/dev/kfd".into()),
                cgroup_permissions: Some("rwm".into()),
            });
            result.devices.push(bollard::models::DeviceMapping {
                path_on_host: Some("/dev/dri".into()),
                path_in_container: Some("/dev/dri".into()),
                cgroup_permissions: Some("rwm".into()),
            });
            // Container needs video + render group access. Use GIDs
            // because containers often lack the group name definitions.
            // Standard GIDs: video=39 (or from /dev/kfd), render=105 (or from renderD128).
            let video_gid = device_group_id("/dev/kfd").unwrap_or(39);
            let render_gid = device_group_id("/dev/dri/renderD128").unwrap_or(105);
            result.group_add.push(video_gid.to_string());
            result.group_add.push(render_gid.to_string());
        }
        _ => {
            // nvidia (default): use Docker device_requests (--gpus)
            result.device_requests.push(bollard::models::DeviceRequest {
                count: Some(gpu.count as i64),
                driver: Some("nvidia".into()),
                capabilities: Some(vec![vec!["gpu".into()]]),
                ..Default::default()
            });
        }
    }

    result
}

/// Get the owning group ID of a device file (e.g. /dev/kfd → 39).
fn device_group_id(path: &str) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| m.gid())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Parse resource limits from the workload spec into Docker host config values.
pub(super) fn parse_resource_limits(spec: &WorkloadSpec) -> (Option<i64>, Option<i64>) {
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

pub(super) fn build_log_config() -> HostConfigLogConfig {
    let mut config = HashMap::new();
    config.insert("max-size".to_string(), "10m".to_string());
    config.insert("max-file".to_string(), "3".to_string());
    HostConfigLogConfig {
        typ: Some("json-file".to_string()),
        config: Some(config),
    }
}

pub(super) fn build_labels(spec: &WorkloadSpec) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    labels.insert(ORCA_LABEL.to_string(), "true".to_string());
    labels.insert("orca.service".to_string(), spec.name.clone());
    if let Some(net) = &spec.network {
        labels.insert("orca.network".to_string(), net.clone());
    }
    // Stamp the domain and container port directly onto the container so
    // a node-local proxy on a joined node can rebuild its route table
    // by inspecting docker labels — no separate state file needed.
    if let Some(domain) = &spec.domain {
        labels.insert("orca.domain".to_string(), domain.clone());
    }
    if let Some(port) = spec.port {
        labels.insert("orca.port".to_string(), port.to_string());
    }
    // Path pattern and strip-prefix let the node-local proxy reconstruct
    // path-based routing (e.g. /admin/* → admin, /* → storefront) from
    // container labels alone. Stored as a comma-joined list for routes
    // and the raw string for strip_prefix.
    if !spec.routes.is_empty() {
        labels.insert("orca.routes".to_string(), spec.routes.join(","));
    }
    if let Some(sp) = &spec.strip_prefix {
        labels.insert("orca.strip_prefix".to_string(), sp.clone());
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
            cmd: vec![],
            extra_ports: vec![],
            strip_prefix: None,
            pull_policy: Default::default(),
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

    #[test]
    fn gpu_nvidia_generates_device_request() {
        use orca_core::types::GpuSpec;
        let mut spec = minimal_spec();
        spec.resources = Some(ResourceLimits {
            memory: None,
            cpu: None,
            gpu: Some(GpuSpec {
                count: 2,
                vendor: Some("nvidia".to_string()),
                vram_min: None,
                model: None,
            }),
        });
        let gpu = build_gpu_passthrough(&spec);
        assert_eq!(gpu.device_requests.len(), 1);
        assert_eq!(gpu.device_requests[0].count, Some(2));
        assert_eq!(gpu.device_requests[0].driver.as_deref(), Some("nvidia"));
        assert!(gpu.devices.is_empty());
        assert!(gpu.group_add.is_empty());
    }

    #[test]
    fn gpu_amd_generates_device_mappings() {
        use orca_core::types::GpuSpec;
        let mut spec = minimal_spec();
        spec.resources = Some(ResourceLimits {
            memory: None,
            cpu: None,
            gpu: Some(GpuSpec {
                count: 1,
                vendor: Some("amd".to_string()),
                vram_min: None,
                model: None,
            }),
        });
        let gpu = build_gpu_passthrough(&spec);
        assert!(gpu.device_requests.is_empty());
        assert_eq!(gpu.devices.len(), 2);
        assert_eq!(gpu.devices[0].path_on_host.as_deref(), Some("/dev/kfd"));
        assert_eq!(gpu.devices[1].path_on_host.as_deref(), Some("/dev/dri"));
        assert_eq!(gpu.group_add.len(), 2);
    }

    #[test]
    fn no_gpu_returns_empty() {
        let spec = minimal_spec();
        let gpu = build_gpu_passthrough(&spec);
        assert!(gpu.device_requests.is_empty());
        assert!(gpu.devices.is_empty());
        assert!(gpu.group_add.is_empty());
    }
}
