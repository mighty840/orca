use std::collections::HashMap;

/// An action the scheduler wants the control plane to execute.
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleAction {
    /// Place a replica on a node.
    Assign {
        service: String,
        replica_idx: u32,
        node_id: u64,
    },
    /// Remove a replica from a node.
    Unassign {
        service: String,
        replica_idx: u32,
        node_id: u64,
    },
}

/// Snapshot of a node's available capacity, used for scheduling decisions.
#[derive(Debug, Clone)]
pub struct NodeCapacity {
    pub node_id: u64,
    pub cpu_available: f64,
    pub memory_available: u64,
    pub gpu_count: u32,
    pub gpu_vram_available: u64,
    pub has_wasm_runtime: bool,
    pub labels: HashMap<String, String>,
    pub current_workload_count: u32,
}
