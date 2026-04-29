//! Workload status reporting and per-container stats collection for the agent.

use orca_core::runtime::Runtime;
use orca_core::types::WorkloadStatus;

use super::AgentClient;
use super::WorkloadInfo;

impl AgentClient {
    /// Collect workload status reports with per-container stats for heartbeat.
    pub async fn collect_workload_reports(
        &self,
        runtime: &dyn Runtime,
    ) -> Vec<orca_core::ws_types::WorkloadReport> {
        let workloads = self.workloads.read().await;
        let mut reports = Vec::with_capacity(workloads.len());

        for (id, info) in workloads.iter() {
            let (cpu, mem) = if info.status == WorkloadStatus::Running {
                let handle = orca_core::runtime::WorkloadHandle {
                    runtime_id: id.clone(),
                    name: format!("orca-{}", info.service_name),
                    metadata: Default::default(),
                };
                match runtime.stats(&handle).await {
                    Ok(rs) => (rs.cpu_percent, rs.memory_bytes),
                    Err(_) => (0.0, 0),
                }
            } else {
                (0.0, 0)
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
