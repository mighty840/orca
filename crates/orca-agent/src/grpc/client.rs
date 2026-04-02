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
}

/// Local tracking info for a workload on this agent.
#[derive(Debug, Clone, Serialize)]
struct WorkloadInfo {
    service_name: String,
    status: WorkloadStatus,
}

/// Heartbeat request sent to the leader.
#[derive(Debug, Serialize)]
struct HeartbeatRequest {
    node_id: u64,
    workloads: Vec<WorkloadStatusReport>,
}

/// Status report for a single workload.
#[derive(Debug, Serialize)]
struct WorkloadStatusReport {
    service_name: String,
    status: String,
}

/// Heartbeat response from the leader.
#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    /// Commands to execute (deploy/remove workloads).
    pub commands: Vec<WorkloadCommand>,
}

/// A command from the leader to the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadCommand {
    /// Action to perform: "deploy" or "stop".
    pub action: String,
    /// Full workload specification for deployment.
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

    /// Run the heartbeat loop with a container runtime for executing commands.
    ///
    /// Uses exponential backoff on failure (5s to 60s), resets on success.
    pub async fn run_heartbeat_loop(&self, interval: Duration, runtime: Arc<dyn Runtime>) {
        const MIN_BACKOFF: Duration = Duration::from_secs(5);
        const MAX_BACKOFF: Duration = Duration::from_secs(60);

        info!(
            "Starting heartbeat loop (interval: {}s)",
            interval.as_secs()
        );

        let mut current_interval = interval;
        let mut was_failing = false;

        loop {
            tokio::time::sleep(current_interval).await;
            match self.heartbeat().await {
                Ok(resp) => {
                    if was_failing {
                        info!("Heartbeat reconnected to leader");
                        was_failing = false;
                    }
                    current_interval = interval;
                    for cmd in resp.commands {
                        info!("Executing command: {} for {}", cmd.action, cmd.spec.name);
                        self.execute_command(&cmd, runtime.as_ref()).await;
                    }
                }
                Err(e) => {
                    was_failing = true;
                    warn!("Heartbeat failed: {e}");
                    current_interval = (current_interval * 2).max(MIN_BACKOFF).min(MAX_BACKOFF);
                }
            }
        }
    }

    /// Execute a workload command received from the leader.
    async fn execute_command(&self, cmd: &WorkloadCommand, runtime: &dyn Runtime) {
        match cmd.action.as_str() {
            "deploy" => {
                info!("Deploying workload: {}", cmd.spec.name);
                match runtime.create(&cmd.spec).await {
                    Ok(handle) => {
                        if let Err(e) = runtime.start(&handle).await {
                            warn!("Failed to start {}: {e}", cmd.spec.name);
                            return;
                        }
                        self.update_workload_status(
                            &handle.runtime_id,
                            &cmd.spec.name,
                            WorkloadStatus::Running,
                        )
                        .await;
                        info!("Workload {} deployed successfully", cmd.spec.name);
                    }
                    Err(e) => {
                        warn!("Failed to create {}: {e}", cmd.spec.name);
                    }
                }
            }
            "stop" => {
                info!("Stop command for {} (not yet implemented)", cmd.spec.name);
            }
            other => {
                warn!("Unknown command action: {other}");
            }
        }
    }

    /// Update local workload status tracking.
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
mod tests {
    use std::time::Duration;

    /// Verify the exponential backoff logic: starts at 5s, doubles, capped at 60s.
    #[test]
    fn test_backoff_doubles() {
        let min_backoff = Duration::from_secs(5);
        let max_backoff = Duration::from_secs(60);

        // Simulate the backoff calculation from run_heartbeat_loop
        let mut interval = min_backoff;
        assert_eq!(interval, Duration::from_secs(5));

        interval = (interval * 2).max(min_backoff).min(max_backoff);
        assert_eq!(interval, Duration::from_secs(10));

        interval = (interval * 2).max(min_backoff).min(max_backoff);
        assert_eq!(interval, Duration::from_secs(20));

        interval = (interval * 2).max(min_backoff).min(max_backoff);
        assert_eq!(interval, Duration::from_secs(40));

        interval = (interval * 2).max(min_backoff).min(max_backoff);
        assert_eq!(interval, Duration::from_secs(60), "should be capped at 60s");

        // Further doublings should stay at 60s
        interval = (interval * 2).max(min_backoff).min(max_backoff);
        assert_eq!(interval, Duration::from_secs(60), "should remain at cap");
    }
}
