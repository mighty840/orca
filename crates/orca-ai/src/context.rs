use orca_core::types::GpuStats;
use serde::Serialize;

/// Structured context snapshot fed to the LLM for diagnosis.
/// The context builder gathers this from the cluster state, then serializes
/// it into the system prompt so the LLM has everything it needs.
#[derive(Debug, Clone, Serialize)]
pub struct ClusterContext {
    pub cluster_name: String,
    pub nodes: Vec<NodeSummary>,
    pub services: Vec<ServiceSummary>,
    pub recent_events: Vec<String>,
    pub active_alerts: Vec<AlertSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeSummary {
    pub id: String,
    pub address: String,
    pub status: String,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub gpu_summary: Vec<GpuSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuSummary {
    pub index: u32,
    pub model: String,
    pub utilization: f64,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub temperature: Option<f64>,
}

impl From<&GpuStats> for GpuSummary {
    fn from(s: &GpuStats) -> Self {
        Self {
            index: s.index,
            model: String::new(),
            utilization: s.utilization,
            vram_used_mb: s.vram_used / (1024 * 1024),
            vram_total_mb: s.vram_total / (1024 * 1024),
            temperature: s.temperature,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceSummary {
    pub name: String,
    pub runtime: String,
    pub replicas_running: u32,
    pub replicas_desired: u32,
    pub status: String,
    pub uses_gpu: bool,
    pub recent_logs: Vec<String>,
    pub error_count_1h: u64,
    pub restart_count_24h: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AlertSummary {
    pub id: String,
    pub service: String,
    pub severity: String,
    pub state: String,
    pub last_message: String,
}

impl ClusterContext {
    /// Render the context into a concise text block for the LLM system prompt.
    pub fn to_system_prompt(&self) -> String {
        let mut out = String::with_capacity(4096);

        out.push_str(&format!(
            "You are Orca AI, the operations assistant for cluster '{}'.\n",
            self.cluster_name
        ));
        out.push_str("You have access to real-time cluster state. Diagnose issues, suggest fixes as `orca` CLI commands, and explain your reasoning.\n");
        out.push_str("When suggesting fixes, output the exact command. When unsure, say so.\n\n");

        out.push_str("## Nodes\n");
        for n in &self.nodes {
            out.push_str(&format!(
                "- {} ({}) status={} cpu={:.0}% mem={:.0}%",
                n.id, n.address, n.status, n.cpu_percent, n.memory_percent
            ));
            for gpu in &n.gpu_summary {
                out.push_str(&format!(
                    " gpu{}={} util={:.0}% vram={}/{}MB temp={}C",
                    gpu.index,
                    gpu.model,
                    gpu.utilization,
                    gpu.vram_used_mb,
                    gpu.vram_total_mb,
                    gpu.temperature.map_or("?".into(), |t| format!("{t:.0}"))
                ));
            }
            out.push('\n');
        }

        out.push_str("\n## Services\n");
        for s in &self.services {
            out.push_str(&format!(
                "- {} [{}] {}/{} replicas, status={}, errors_1h={}, restarts_24h={}",
                s.name,
                s.runtime,
                s.replicas_running,
                s.replicas_desired,
                s.status,
                s.error_count_1h,
                s.restart_count_24h,
            ));
            if s.uses_gpu {
                out.push_str(" [GPU]");
            }
            out.push('\n');
            for log in s.recent_logs.iter().take(5) {
                out.push_str(&format!("    {log}\n"));
            }
        }

        if !self.active_alerts.is_empty() {
            out.push_str("\n## Active Alerts\n");
            for a in &self.active_alerts {
                out.push_str(&format!(
                    "- [{}] {} ({}): {} — {}\n",
                    a.severity, a.service, a.state, a.id, a.last_message
                ));
            }
        }

        if !self.recent_events.is_empty() {
            out.push_str("\n## Recent Events\n");
            for e in self.recent_events.iter().take(20) {
                out.push_str(&format!("- {e}\n"));
            }
        }

        out
    }
}
