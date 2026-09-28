//! API client for fetching cluster data from the orca control plane.

use std::collections::HashMap;

use serde::Deserialize;

mod cluster;
mod events;

pub use cluster::BackupTriggerTarget;

pub use orca_core::api_types::{
    ClusterBackupsResponse, ClusterNetworksResponse, DockerNetwork, DomainRoute, FailureInfo,
    LastBackupResult, NetworkService, NodeBackupStatus, NodeNetworks, NodeRole, SecretRef,
    SecretUsage, SecretsUsageResponse, TriggerBackupResponse, WebhookEntry, WebhookInvocation,
    WebhookInvocationsResponse, WebhookListResponse,
};
pub use orca_core::types::{AlertConversation, AlertSender, AlertSeverity, AlertState};

/// Fetches cluster data from the orca API.
/// Cheaply cloneable so background tasks (chat dispatch) can own a handle
/// without sharing references with the event loop.
#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    client: reqwest::Client,
    /// For long-lived log streams: no overall timeout, which would cut a
    /// followed log off after 10 s.
    stream_client: reqwest::Client,
    token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusResponse {
    pub cluster_name: String,
    pub services: Vec<ServiceStatus>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ServiceStatus {
    pub name: String,
    #[serde(default)]
    pub image: String,
    pub runtime: String,
    pub desired_replicas: u32,
    pub running_replicas: u32,
    pub status: String,
    pub domain: Option<String>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub memory_usage: Option<String>,
    #[serde(default)]
    pub cpu_percent: Option<f64>,
    /// Node this service runs on. `None` means the master.
    #[serde(default)]
    pub node: Option<String>,
    /// Configured memory limit in bytes, used to scale the sparkline.
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
    /// Why the service is degraded/stopped, if known (deploy error or crash).
    #[serde(default)]
    pub last_failure: Option<FailureInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ClusterInfo {
    pub cluster_name: String,
    pub node_count: u64,
    pub nodes: Vec<NodeInfo>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeInfo {
    pub node_id: u64,
    pub address: String,
    pub last_heartbeat: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub drain: bool,
    /// Latest reported CPU percent (0..100). 0 if the node hasn't reported.
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub memory_bytes: u64,
    #[serde(default)]
    pub memory_total: u64,
    #[serde(default)]
    pub disk_used: u64,
    #[serde(default)]
    pub disk_total: u64,
    #[serde(default)]
    pub net_rx: u64,
    #[serde(default)]
    pub net_tx: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecretListResponse {
    pub keys: Vec<String>,
}

/// A one-shot action on a service; the CLI's verb of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Redeploy,
    Rollback,
    Promote,
}

impl ServiceAction {
    pub fn verb(self) -> &'static str {
        match self {
            ServiceAction::Start => "start",
            ServiceAction::Redeploy => "redeploy",
            ServiceAction::Rollback => "rollback",
            ServiceAction::Promote => "promote",
        }
    }
}

impl ApiClient {
    /// Get the base URL for display purposes.
    pub fn url(&self) -> &str {
        &self.base_url
    }

    pub fn new(base_url: &str) -> Self {
        let token = std::env::var("ORCA_TOKEN").ok().or_else(|| {
            let home = std::env::var("HOME").ok()?;
            std::fs::read_to_string(format!("{home}/.orca/cluster.token"))
                .ok()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
        });
        // Aggressive timeouts: every TUI call runs inside the event loop,
        // so a slow/dead server can hang the whole UI (no key handling, no
        // Ctrl+C). Original symptom was "TUI stuck after pressing 7 and
        // Ctrl+C doesn't help" — refresh() was awaiting forever on a
        // stalled HTTP call. Short caps keep the loop alive.
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("build reqwest client");
        let stream_client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .build()
            .expect("build reqwest stream client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            stream_client,
            token,
        }
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(t) = &self.token {
            req.bearer_auth(t)
        } else {
            req
        }
    }

    pub async fn status(&self) -> anyhow::Result<StatusResponse> {
        let resp = self
            .auth(self.client.get(format!("{}/api/v1/status", self.base_url)))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn status_filtered(&self, project: &str) -> anyhow::Result<StatusResponse> {
        let resp = self
            .auth(
                self.client
                    .get(format!("{}/api/v1/status?project={project}", self.base_url)),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn cluster_info(&self) -> anyhow::Result<ClusterInfo> {
        let resp = self
            .auth(
                self.client
                    .get(format!("{}/api/v1/cluster/info", self.base_url)),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn logs(&self, service: &str, tail: u64) -> anyhow::Result<String> {
        let resp = self
            .auth(self.client.get(format!(
                "{}/api/v1/services/{service}/logs?tail={tail}&follow=false",
                self.base_url
            )))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    /// Follow a service's logs: the response body grows as the service
    /// writes. For a service on an agent the master answers with one batch
    /// and closes, so the caller falls back to polling when the body ends.
    pub async fn logs_follow(&self, service: &str, tail: u64) -> anyhow::Result<reqwest::Response> {
        Ok(self
            .auth(self.stream_client.get(format!(
                "{}/api/v1/services/{service}/logs?tail={tail}&follow=true",
                self.base_url
            )))
            .send()
            .await?
            .error_for_status()?)
    }

    pub async fn stop(&self, service: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .delete(format!("{}/api/v1/services/{service}", self.base_url)),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn stop_project(&self, project: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .delete(format!("{}/api/v1/projects/{project}", self.base_url)),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    /// `POST /api/v1/services/{service}/{action}` for `start` (resume a
    /// paused service), `redeploy`, `rollback` and `promote`.
    pub async fn service_action(&self, service: &str, action: ServiceAction) -> anyhow::Result<()> {
        let verb = action.verb();
        self.auth(self.client.post(format!(
            "{}/api/v1/services/{service}/{verb}",
            self.base_url
        )))
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn scale(&self, service: &str, replicas: u32) -> anyhow::Result<()> {
        self.auth(
            self.client
                .post(format!("{}/api/v1/services/{service}/scale", self.base_url))
                .json(&serde_json::json!({"replicas": replicas})),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn metrics(&self) -> anyhow::Result<String> {
        let resp = self
            .auth(self.client.get(format!("{}/metrics", self.base_url)))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    pub async fn drain(&self, node_id: u64) -> anyhow::Result<()> {
        self.auth(self.client.post(format!(
            "{}/api/v1/cluster/nodes/{node_id}/drain",
            self.base_url
        )))
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn undrain(&self, node_id: u64) -> anyhow::Result<()> {
        self.auth(self.client.post(format!(
            "{}/api/v1/cluster/nodes/{node_id}/undrain",
            self.base_url
        )))
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    /// POST a chat turn to `/api/v1/ask`. Returns `Ok(None)` when the server
    /// has no `[ai]` configured (503), so the caller renders a placeholder
    /// rather than treating it as a hard error.
    pub async fn ask(
        &self,
        question: &str,
        history: &[(String, String)],
    ) -> anyhow::Result<Option<String>> {
        let url = format!("{}/api/v1/ask", self.base_url);
        let history_json: Vec<serde_json::Value> = history
            .iter()
            .map(|(role, content)| serde_json::json!({ "role": role, "content": content }))
            .collect();
        let body = serde_json::json!({
            "question": question,
            "history": history_json,
        });
        let resp = self.auth(self.client.post(&url)).json(&body).send().await?;
        if resp.status().as_u16() == 503 {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        #[derive(Deserialize)]
        struct AskResp {
            response: String,
        }
        let body: AskResp = resp.json().await?;
        Ok(Some(body.response))
    }
}
