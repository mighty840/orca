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

        // Authoritative command surface. The LLM otherwise invents plausible
        // but non-existent commands like `orca service restart` (this CLI is
        // flat — verbs are top-level, not nested under `service`).
        out.push_str("## Available `orca` commands\n");
        out.push_str("Use ONLY these. Do NOT invent subcommands.\n\n");
        out.push_str("- `orca status` — cluster + service overview\n");
        out.push_str("- `orca logs <service> [--tail N] [--summarize]` — tail logs; `--summarize` runs them through the AI\n");
        out.push_str("- `orca redeploy <service>` — force fresh image pull + container recreate (this is the 'restart' verb)\n");
        out.push_str("- `orca rollback <service>` — roll back to the previous successful deploy\n");
        out.push_str("- `orca scale <service> --replicas N` — change replica count\n");
        out.push_str("- `orca stop <service>` — stop a service (omit for all)\n");
        out.push_str("- `orca promote <service>` — promote canary instances to stable\n");
        out.push_str("- `orca exec <service> [cmd]` — interactive shell or one-shot command inside a running container\n");
        out.push_str("- `orca secrets set <KEY> <VALUE>` — set a secret referenced as `${secrets.KEY}` in service.toml env\n");
        out.push_str("- `orca secrets list` / `orca secrets get <KEY>` / `orca secrets remove <KEY>` — read/remove secrets\n");
        out.push_str("- `orca deploy [service…]` — (re)apply service definitions from `services/`\n");
        out.push_str("- `orca alerts list` / `orca alerts view <id>` / `orca alerts reply <id> <msg>` / `orca alerts dismiss|resolve <id>` — alert triage\n");
        out.push_str("- `orca backup` / `orca cleanup` / `orca nodes` — operational utilities\n\n");
        out.push_str("**Not supported (do not suggest):** `orca service <verb>` (the CLI is flat — there is no `service` subcommand). ");
        out.push_str("There is also no `set-env` / `update --cmd` / `update --image` / `update --port` — env vars, image tag, command, and ports live in `services/<project>/service.toml` and apply on `orca deploy`. ");
        out.push_str("If a fix requires editing a service definition, say so plainly (e.g. 'edit services/<project>/service.toml: change `image = ...`, then `orca deploy`').\n\n");

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_summary_from_converts_bytes_to_mb() {
        let stats = GpuStats {
            index: 0,
            utilization: 75.0,
            vram_used: 4 * 1024 * 1024 * 1024,  // 4 GiB in bytes
            vram_total: 8 * 1024 * 1024 * 1024, // 8 GiB in bytes
            temperature: Some(65.0),
            power_watts: None,
        };
        let summary = GpuSummary::from(&stats);
        assert_eq!(summary.vram_used_mb, 4096);
        assert_eq!(summary.vram_total_mb, 8192);
        assert_eq!(summary.index, 0);
        assert!((summary.utilization - 75.0).abs() < f64::EPSILON);
        assert_eq!(summary.temperature, Some(65.0));
    }

    #[test]
    fn test_cluster_context_system_prompt_contains_sections() {
        let ctx = ClusterContext {
            cluster_name: "test-cluster".to_string(),
            nodes: vec![NodeSummary {
                id: "node-1".to_string(),
                address: "10.0.0.1".to_string(),
                status: "healthy".to_string(),
                cpu_percent: 42.0,
                memory_percent: 60.0,
                gpu_summary: vec![],
            }],
            services: vec![ServiceSummary {
                name: "api".to_string(),
                runtime: "container".to_string(),
                replicas_running: 2,
                replicas_desired: 3,
                status: "degraded".to_string(),
                uses_gpu: false,
                recent_logs: vec![],
                error_count_1h: 5,
                restart_count_24h: 1,
            }],
            recent_events: vec!["node-1 joined".to_string()],
            active_alerts: vec![],
        };
        let prompt = ctx.to_system_prompt();
        assert!(prompt.contains("test-cluster"));
        assert!(prompt.contains("## Nodes"));
        assert!(prompt.contains("## Services"));
        assert!(prompt.contains("## Recent Events"));
        assert!(prompt.contains("node-1"));
        assert!(prompt.contains("api"));
    }

    #[test]
    fn test_system_prompt_includes_command_reference() {
        let ctx = ClusterContext {
            cluster_name: "x".into(),
            nodes: vec![],
            services: vec![],
            recent_events: vec![],
            active_alerts: vec![],
        };
        let prompt = ctx.to_system_prompt();
        assert!(prompt.contains("## Available `orca` commands"));
        assert!(prompt.contains("orca redeploy"));
        assert!(prompt.contains("orca logs"));
        assert!(prompt.contains("orca rollback"));
        assert!(prompt.contains("Not supported"));
        // The hallucinated forms must be called out as wrong.
        assert!(prompt.contains("`orca service <verb>`"));
    }
}
