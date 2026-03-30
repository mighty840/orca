use std::collections::HashMap;

use orca_core::types::RuntimeKind;

use super::types::{NodeCapacity, ScheduleAction};

/// A request describing the desired state for one service.
#[derive(Debug, Clone)]
pub struct ServiceRequest {
    pub name: String,
    pub replicas_desired: u32,
    pub runtime: RuntimeKind,
    pub cpu_required: f64,
    pub memory_required: u64,
    pub gpu_required: u32,
    pub placement_labels: HashMap<String, String>,
    pub requires_gpu: bool,
}

/// A current replica assignment.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub service: String,
    pub replica_idx: u32,
    pub node_id: u64,
}

/// Pure scheduling function. Given the desired services, available nodes, and
/// current assignments, returns the list of actions to converge toward the
/// desired state.
pub fn schedule(
    services: &[ServiceRequest],
    nodes: &[NodeCapacity],
    assignments: &[Assignment],
) -> Vec<ScheduleAction> {
    let mut actions = Vec::new();

    for svc in services {
        let current: Vec<&Assignment> = assignments
            .iter()
            .filter(|a| a.service == svc.name)
            .collect();
        let current_count = current.len() as u32;
        let desired = svc.replicas_desired;

        if desired > current_count {
            // Scale up: find next replica indices and assign them.
            let used_indices: Vec<u32> = current.iter().map(|a| a.replica_idx).collect();
            let assigned_nodes: Vec<u64> = current.iter().map(|a| a.node_id).collect();
            let new_count = desired - current_count;

            let mut next_idx = 0u32;
            let mut placed = 0u32;
            while placed < new_count {
                // Find next unused replica index.
                while used_indices.contains(&next_idx) {
                    next_idx += 1;
                }

                if let Some(node_id) = pick_best_node(nodes, svc, &assigned_nodes, &actions) {
                    actions.push(ScheduleAction::Assign {
                        service: svc.name.clone(),
                        replica_idx: next_idx,
                        node_id,
                    });
                }
                next_idx += 1;
                placed += 1;
            }
        } else if desired < current_count {
            // Scale down: remove replicas from the most-loaded nodes first.
            let remove_count = current_count - desired;
            let mut removable: Vec<&Assignment> = current.clone();
            removable.sort_by(|a, b| {
                let load_a = node_workload_count(nodes, a.node_id);
                let load_b = node_workload_count(nodes, b.node_id);
                load_b.cmp(&load_a) // most-loaded first
            });

            for assignment in removable.iter().take(remove_count as usize) {
                actions.push(ScheduleAction::Unassign {
                    service: assignment.service.clone(),
                    replica_idx: assignment.replica_idx,
                    node_id: assignment.node_id,
                });
            }
        }
    }

    actions
}

/// Pick the best node for a service replica by scoring all eligible nodes.
fn pick_best_node(
    nodes: &[NodeCapacity],
    svc: &ServiceRequest,
    already_assigned: &[u64],
    pending_actions: &[ScheduleAction],
) -> Option<u64> {
    let mut best_score = 0.0_f64;
    let mut best_node = None;

    for node in nodes {
        let score = score_node(node, svc, already_assigned, pending_actions);
        if score > best_score {
            best_score = score;
            best_node = Some(node.node_id);
        }
    }

    best_node
}

/// Score a node for a service. Returns 0.0 if the node is ineligible.
fn score_node(
    node: &NodeCapacity,
    svc: &ServiceRequest,
    already_assigned: &[u64],
    pending_actions: &[ScheduleAction],
) -> f64 {
    // Hard constraints: resources.
    if node.cpu_available < svc.cpu_required {
        return 0.0;
    }
    if node.memory_available < svc.memory_required {
        return 0.0;
    }
    if svc.requires_gpu && node.gpu_count < svc.gpu_required.max(1) {
        return 0.0;
    }

    // Hard constraint: placement labels must all match.
    for (key, value) in &svc.placement_labels {
        match node.labels.get(key) {
            Some(v) if v == value => {}
            _ => return 0.0,
        }
    }

    // Base score: weighted sum of available resources (normalized loosely).
    let mut score = node.cpu_available * 10.0 + node.memory_available as f64 / 1_000_000.0;

    // Bonus for wasm-capable nodes when the service is wasm.
    if svc.runtime == RuntimeKind::Wasm && node.has_wasm_runtime {
        score += 50.0;
    }

    // Penalty if this node already runs a replica of the same service (spread).
    let existing_count = already_assigned
        .iter()
        .filter(|&&id| id == node.node_id)
        .count();
    let pending_count = pending_actions
        .iter()
        .filter(|a| matches!(a, ScheduleAction::Assign { node_id, service, .. } if *node_id == node.node_id && *service == svc.name))
        .count();
    let total_same = existing_count + pending_count;
    score -= total_same as f64 * 100.0;

    // Penalty for overall node load.
    score -= node.current_workload_count as f64 * 5.0;

    score
}

/// Look up a node's current workload count.
fn node_workload_count(nodes: &[NodeCapacity], node_id: u64) -> u32 {
    nodes
        .iter()
        .find(|n| n.node_id == node_id)
        .map_or(0, |n| n.current_workload_count)
}

#[cfg(test)]
#[path = "algo_tests.rs"]
mod tests;
