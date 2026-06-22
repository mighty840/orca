//! Workload status reporting and per-container stats collection for the agent.

use tokio::io::AsyncReadExt;

use orca_core::runtime::{LogOpts, Runtime, WorkloadHandle};
use orca_core::types::WorkloadStatus;

use super::AgentClient;
use super::WorkloadInfo;

/// Max bytes of log tail to ship in a heartbeat for a crashed container.
const LOG_TAIL_LIMIT: usize = 4096;

/// Fetch the last few log lines of a (typically crashed) container so the
/// master can explain the failure K8s-style. Returns `None` on error/empty.
async fn read_log_tail(runtime: &dyn Runtime, handle: &WorkloadHandle) -> Option<String> {
    let opts = LogOpts {
        follow: false,
        tail: Some(20),
        since: None,
        timestamps: false,
    };
    let mut stream = runtime.logs(handle, &opts).await.ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.ok()?;
    let text = String::from_utf8_lossy(&buf);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Keep the most recent bytes if oversized, so the heartbeat stays small.
    let out = if trimmed.len() > LOG_TAIL_LIMIT {
        format!("…{}", &trimmed[trimmed.len() - LOG_TAIL_LIMIT..])
    } else {
        trimmed.to_string()
    };
    Some(out)
}

impl AgentClient {
    /// Collect workload status reports with per-container stats for heartbeat.
    pub async fn collect_workload_reports(
        &self,
        runtime: &dyn Runtime,
    ) -> Vec<orca_core::ws_types::WorkloadReport> {
        let workloads = self.workloads.read().await;
        let mut reports = Vec::with_capacity(workloads.len());

        for (id, info) in workloads.iter() {
            let handle = WorkloadHandle {
                runtime_id: id.clone(),
                name: format!("orca-{}", info.service_name),
                metadata: Default::default(),
            };
            // Running: collect live stats. Not running: collect crash detail
            // (exit code, restart count, log tail) so the master can explain
            // *why* without a separate fetch.
            let (cpu, mem, exit_code, restart_count, last_logs) =
                if info.status == WorkloadStatus::Running {
                    let (cpu, mem) = match runtime.stats(&handle).await {
                        Ok(rs) => (rs.cpu_percent, rs.memory_bytes),
                        Err(_) => (0.0, 0),
                    };
                    (cpu, mem, None, 0, None)
                } else {
                    let exit = runtime.last_exit(&handle).await.unwrap_or_default();
                    let logs = read_log_tail(runtime, &handle).await;
                    (0.0, 0, exit.exit_code, exit.restart_count, logs)
                };

            reports.push(orca_core::ws_types::WorkloadReport {
                service_name: info.service_name.clone(),
                status: match info.status {
                    WorkloadStatus::Running => "running",
                    WorkloadStatus::Stopped | WorkloadStatus::Completed => "stopped",
                    WorkloadStatus::Failed => "failed",
                    WorkloadStatus::Pending | WorkloadStatus::Creating => "pending",
                    WorkloadStatus::Stopping => "stopping",
                }
                .into(),
                container_id: Some(id.clone()),
                cpu_percent: cpu,
                memory_bytes: mem,
                exit_code,
                restart_count,
                last_logs,
            });
        }

        reports
    }

    pub async fn update_workload_status(&self, id: &str, service: &str, status: WorkloadStatus) {
        let mut workloads = self.workloads.write().await;
        workloads.insert(
            id.to_string(),
            WorkloadInfo {
                service_name: service.to_string(),
                status,
            },
        );
    }
}
