use std::collections::HashMap;

use orca_core::types::RuntimeKind;

use super::*;

fn make_node(id: u64, cpu: f64, mem: u64, workloads: u32) -> NodeCapacity {
    NodeCapacity {
        node_id: id,
        cpu_available: cpu,
        memory_available: mem,
        gpu_count: 0,
        gpu_vram_available: 0,
        has_wasm_runtime: false,
        labels: HashMap::new(),
        current_workload_count: workloads,
    }
}

fn make_service(name: &str, replicas: u32) -> ServiceRequest {
    ServiceRequest {
        name: name.to_string(),
        replicas_desired: replicas,
        runtime: RuntimeKind::Container,
        cpu_required: 1.0,
        memory_required: 512,
        gpu_required: 0,
        placement_labels: HashMap::new(),
        requires_gpu: false,
    }
}

#[test]
fn spreads_replicas_across_nodes() {
    let nodes = vec![
        make_node(1, 4.0, 4096, 0),
        make_node(2, 4.0, 4096, 0),
        make_node(3, 4.0, 4096, 0),
    ];
    let svc = make_service("web", 3);
    let actions = schedule(&[svc], &nodes, &[]);

    let mut assigned_nodes: Vec<u64> = actions
        .iter()
        .filter_map(|a| match a {
            ScheduleAction::Assign { node_id, .. } => Some(*node_id),
            _ => None,
        })
        .collect();
    assigned_nodes.sort();
    assert_eq!(assigned_nodes, vec![1, 2, 3]);
}

#[test]
fn gpu_requirement_filters_nodes() {
    let mut gpu_node = make_node(1, 4.0, 4096, 0);
    gpu_node.gpu_count = 2;
    gpu_node.gpu_vram_available = 16_000;

    let cpu_node = make_node(2, 8.0, 8192, 0);

    let svc = ServiceRequest {
        name: "ml-train".to_string(),
        replicas_desired: 1,
        runtime: RuntimeKind::Container,
        cpu_required: 1.0,
        memory_required: 512,
        gpu_required: 1,
        placement_labels: HashMap::new(),
        requires_gpu: true,
    };

    let actions = schedule(&[svc], &[gpu_node, cpu_node], &[]);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ScheduleAction::Assign { node_id, .. } => assert_eq!(*node_id, 1),
        _ => panic!("expected Assign action"),
    }
}

#[test]
fn scale_down_removes_from_most_loaded() {
    let nodes = vec![make_node(1, 4.0, 4096, 5), make_node(2, 4.0, 4096, 1)];
    let svc = make_service("api", 1);
    let assignments = vec![
        Assignment {
            service: "api".to_string(),
            replica_idx: 0,
            node_id: 1,
        },
        Assignment {
            service: "api".to_string(),
            replica_idx: 1,
            node_id: 2,
        },
    ];

    let actions = schedule(&[svc], &nodes, &assignments);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ScheduleAction::Unassign { node_id, .. } => assert_eq!(*node_id, 1),
        _ => panic!("expected Unassign action"),
    }
}

#[test]
fn wasm_preference_scores_higher() {
    let mut wasm_node = make_node(1, 4.0, 4096, 0);
    wasm_node.has_wasm_runtime = true;

    let container_node = make_node(2, 4.0, 4096, 0);

    let svc = ServiceRequest {
        name: "edge-fn".to_string(),
        replicas_desired: 1,
        runtime: RuntimeKind::Wasm,
        cpu_required: 0.5,
        memory_required: 256,
        gpu_required: 0,
        placement_labels: HashMap::new(),
        requires_gpu: false,
    };

    let actions = schedule(&[svc], &[wasm_node, container_node], &[]);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ScheduleAction::Assign { node_id, .. } => assert_eq!(*node_id, 1),
        _ => panic!("expected Assign action"),
    }
}
