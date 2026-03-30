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

use orca_core::types::WorkloadStatus;

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
#[derive(Debug, Deserialize)]
pub struct WorkloadCommand {
    /// Action to perform.
    pub action: String,
    /// Service name.
    pub service: String,
    /// Container image or wasm module.
    pub image: String,
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

        let resp = self
            .client
            .post(format!("{}/api/v1/cluster/register", self.leader_url))
            .json(&body)
            .send()
            .await?;

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

        let resp = self
            .client
            .post(format!("{}/api/v1/cluster/heartbeat", self.leader_url))
            .json(&req)
            .send()
            .await?;

        if resp.status().is_success() {
            Ok(resp.json().await?)
        } else {
            debug!("Heartbeat failed: {}", resp.status());
            Ok(HeartbeatResponse {
                commands: Vec::new(),
            })
        }
    }

    /// Run the heartbeat loop (call from a spawned task).
    pub async fn run_heartbeat_loop(&self, interval: Duration) {
        info!(
            "Starting heartbeat loop (interval: {}s)",
            interval.as_secs()
        );
        loop {
            tokio::time::sleep(interval).await;
            match self.heartbeat().await {
                Ok(resp) => {
                    if !resp.commands.is_empty() {
                        info!("Received {} commands from leader", resp.commands.len());
                    }
                }
                Err(e) => {
                    warn!("Heartbeat failed: {e}");
                }
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
