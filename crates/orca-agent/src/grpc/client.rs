//! HTTP-based agent client for communicating with the leader's API.
//!
//! Uses the REST API rather than gRPC for simplicity in M2. The agent
//! registers with the leader, sends periodic heartbeats, and receives
//! workload commands.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use orca_core::runtime::Runtime;
use orca_core::types::{WorkloadSpec, WorkloadStatus};

/// Maximum number of retry attempts for failed commands.
const MAX_COMMAND_RETRIES: u32 = 3;

/// Agent client that communicates with the cluster leader.
pub struct AgentClient {
    /// Leader's API URL.
    leader_url: String,
    /// This node's ID.
    node_id: u64,
    /// HTTP client.
    client: reqwest::Client,
    /// Local workload handles and their status.
    workloads: Arc<RwLock<HashMap<String, WorkloadInfo>>>,
    /// Commands that failed and should be retried (command, attempt_count).
    failed_commands: Arc<RwLock<Vec<(WorkloadCommand, u32)>>>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkloadInfo {
    service_name: String,
    status: WorkloadStatus,
}

#[derive(Debug, Serialize)]
struct HeartbeatRequest {
    node_id: u64,
    workloads: Vec<WorkloadStatusReport>,
}

#[derive(Debug, Serialize)]
struct WorkloadStatusReport {
    service_name: String,
    status: String,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    pub commands: Vec<WorkloadCommand>,
}

/// A command from the leader to the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadCommand {
    pub action: String,
    pub spec: WorkloadSpec,
}

impl AgentClient {
    /// Create a new agent client.
    pub fn new(leader_url: String, node_id: u64) -> Self {
        Self {
            leader_url: leader_url.trim_end_matches('/').to_string(),
            node_id,
            client: reqwest::Client::new(),
            workloads: Arc::new(RwLock::new(HashMap::new())),
            failed_commands: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register this node with the leader.
    pub async fn register(
        &self,
        address: &str,
        labels: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "node_id": self.node_id,
            "address": address,
            "labels": labels,
        });

        let mut req = self
            .client
            .post(format!("{}/api/v1/cluster/register", self.leader_url))
            .json(&body);

        if let Ok(token) = std::env::var("ORCA_TOKEN") {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await?;

        if resp.status().is_success() {
            info!("Registered with leader as node {}", self.node_id);
            Ok(())
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Registration failed (HTTP {status}): {body}")
        }
    }

    /// Send a heartbeat to the leader and receive commands.
    pub async fn heartbeat(&self) -> anyhow::Result<HeartbeatResponse> {
        let workloads = self.workloads.read().await;
        let reports: Vec<WorkloadStatusReport> = workloads
            .values()
            .map(|w| WorkloadStatusReport {
                service_name: w.service_name.clone(),
                status: format!("{:?}", w.status).to_lowercase(),
            })
            .collect();
        drop(workloads);

        let req = HeartbeatRequest {
            node_id: self.node_id,
            workloads: reports,
        };

        let mut hb_req = self
            .client
            .post(format!("{}/api/v1/cluster/heartbeat", self.leader_url))
            .json(&req);

        if let Ok(token) = std::env::var("ORCA_TOKEN") {
            hb_req = hb_req.bearer_auth(token);
        }

        let resp = hb_req.send().await?;

        if resp.status().is_success() {
            Ok(resp.json().await?)
        } else {
            debug!("Heartbeat failed: {}", resp.status());
            Ok(HeartbeatResponse {
                commands: Vec::new(),
            })
        }
    }

    /// Run the heartbeat loop. Backoff: 5s to 60s on failure, resets on success.
    pub async fn run_heartbeat_loop(&self, interval: Duration, runtime: Arc<dyn Runtime>) {
        const MIN_BO: Duration = Duration::from_secs(5);
        const MAX_BO: Duration = Duration::from_secs(60);
        info!(
            "Starting heartbeat loop (interval: {}s)",
            interval.as_secs()
        );
        let (mut cur, mut failing) = (interval, false);
        loop {
            tokio::time::sleep(cur).await;
            match self.heartbeat().await {
                Ok(resp) => {
                    if failing {
                        info!("Heartbeat reconnected to leader");
                    }
                    cur = interval;
                    failing = false;
                    self.retry_failed_commands(runtime.as_ref()).await;
                    for cmd in resp.commands {
                        info!("Executing command: {} for {}", cmd.action, cmd.spec.name);
                        self.run_command(&cmd, runtime.as_ref(), true).await;
                    }
                }
                Err(e) => {
                    failing = true;
                    warn!("Heartbeat failed: {e}");
                    cur = (cur * 2).max(MIN_BO).min(MAX_BO);
                }
            }
        }
    }

    /// Execute a workload command. Returns `true` on success.
    /// When `enqueue_on_fail` is true, failures are added to the retry queue.
    async fn run_command(
        &self,
        cmd: &WorkloadCommand,
        runtime: &dyn Runtime,
        enqueue_on_fail: bool,
    ) -> bool {
        match cmd.action.as_str() {
            "deploy" => match runtime.create(&cmd.spec).await {
                Ok(handle) => {
                    if let Err(e) = runtime.start(&handle).await {
                        warn!("Failed to start {}: {e}", cmd.spec.name);
                        if enqueue_on_fail {
                            self.enqueue_failed_command(cmd.clone()).await;
                        }
                        return false;
                    }
                    self.update_workload_status(
                        &handle.runtime_id,
                        &cmd.spec.name,
                        WorkloadStatus::Running,
                    )
                    .await;
                    info!("Workload {} deployed successfully", cmd.spec.name);
                    true
                }
                Err(e) => {
                    warn!("Failed to create {}: {e}", cmd.spec.name);
                    if enqueue_on_fail {
                        self.enqueue_failed_command(cmd.clone()).await;
                    }
                    false
                }
            },
            "stop" => {
                info!("Stop command for {} (not yet implemented)", cmd.spec.name);
                true
            }
            other => {
                warn!("Unknown command action: {other}");
                true
            }
        }
    }

    /// Add a failed command to the retry queue with attempt count 1.
    async fn enqueue_failed_command(&self, cmd: WorkloadCommand) {
        let mut failed = self.failed_commands.write().await;
        failed.push((cmd, 1));
    }

    /// Retry previously failed commands, dropping any that exceed max retries.
    async fn retry_failed_commands(&self, runtime: &dyn Runtime) {
        let commands: Vec<(WorkloadCommand, u32)> =
            std::mem::take(&mut *self.failed_commands.write().await);
        for (cmd, attempts) in commands {
            info!(
                "Retrying {} for {} (attempt {}/{})",
                cmd.action, cmd.spec.name, attempts, MAX_COMMAND_RETRIES
            );
            if !self.run_command(&cmd, runtime, false).await {
                let next = attempts + 1;
                if next > MAX_COMMAND_RETRIES {
                    warn!(
                        "{} for {} exceeded max retries, dropping",
                        cmd.action, cmd.spec.name
                    );
                    self.update_workload_status(
                        &cmd.spec.name,
                        &cmd.spec.name,
                        WorkloadStatus::Failed,
                    )
                    .await;
                } else {
                    self.failed_commands.write().await.push((cmd, next));
                }
            }
        }
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
#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
