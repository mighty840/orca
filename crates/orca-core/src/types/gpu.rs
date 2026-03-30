use serde::{Deserialize, Serialize};

use super::WorkloadId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuSpec {
    /// Number of GPUs required.
    pub count: u32,
    /// GPU vendor filter (e.g., "nvidia", "amd").
    pub vendor: Option<String>,
    /// Minimum VRAM in MiB.
    pub vram_min: Option<u64>,
    /// Specific GPU model (e.g., "A100", "RTX4090", "H100").
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Device index on this node.
    pub index: u32,
    /// Vendor (nvidia, amd).
    pub vendor: String,
    /// Model name.
    pub model: String,
    /// Total VRAM in bytes.
    pub vram_total: u64,
    /// Used VRAM in bytes.
    pub vram_used: u64,
    /// GPU utilization percentage.
    pub utilization: f64,
    /// Temperature in celsius.
    pub temperature: Option<f64>,
    /// Currently allocated to this workload (if any).
    pub allocated_to: Option<WorkloadId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuStats {
    pub index: u32,
    pub utilization: f64,
    pub vram_used: u64,
    pub vram_total: u64,
    pub temperature: Option<f64>,
    pub power_watts: Option<f64>,
}
