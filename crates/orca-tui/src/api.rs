//! API client for fetching cluster data from the orca control plane.

use std::collections::HashMap;

use serde::Deserialize;

pub use orca_core::api_types::{
    ClusterBackupsResponse, ClusterNetworksResponse, DockerNetwork, DomainRoute, LastBackupResult,
    NetworkService, NodeBackupStatus, NodeNetworks, NodeRole, SecretRef, SecretUsage,
    SecretsUsageResponse, TriggerBackupResponse, WebhookEntry, WebhookInvocation,
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
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
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

    /// Fetch the cluster networks dashboard: per-node `orca-*` Docker bridge
    /// listing plus the master's public-edge route table.
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

    pub async fn cluster_networks(&self) -> anyhow::Result<ClusterNetworksResponse> {
        let resp = self
            .auth(
                self.client
                    .get(format!("{}/api/v1/cluster/networks", self.base_url)),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Fetch the secrets organizer view: every key + the services that
    /// reference it. Computed server-side from `state.services` so the TUI
    /// doesn't need to fetch full service configs.
    pub async fn secrets_usage(&self) -> anyhow::Result<SecretsUsageResponse> {
        let resp = self
            .auth(
                self.client
                    .get(format!("{}/api/v1/secrets/usage", self.base_url)),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn list_secrets(&self) -> anyhow::Result<Vec<String>> {
        let resp = self
            .auth(self.client.get(format!("{}/api/v1/secrets", self.base_url)))
            .send()
            .await?
            .error_for_status()?;
        let body: SecretListResponse = resp.json().await?;
        Ok(body.keys)
    }

    pub async fn set_secret(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .post(format!("{}/api/v1/secrets/{key}", self.base_url))
                .json(&serde_json::json!({"value": value})),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn remove_secret(&self, key: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .delete(format!("{}/api/v1/secrets/{key}", self.base_url)),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    /// Fetch the per-node backup status for the cluster dashboard.
    pub async fn cluster_backups(&self) -> anyhow::Result<ClusterBackupsResponse> {
        let resp = self
            .auth(
                self.client
                    .get(format!("{}/api/v1/cluster/backups", self.base_url)),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Trigger an immediate backup on a single target. `Master` runs the
    /// master's own `orca backup all` subprocess; `Agent(id)` dispatches a
    /// `BackupRequest` via WS to that agent.
    pub async fn trigger_backup(
        &self,
        target: BackupTriggerTarget,
    ) -> anyhow::Result<TriggerBackupResponse> {
        let query = match target {
            BackupTriggerTarget::Master => "master=true".to_string(),
            BackupTriggerTarget::Agent(id) => format!("node_id={id}"),
        };
        let resp = self
            .auth(self.client.post(format!(
                "{}/api/v1/cluster/backups/trigger?{query}",
                self.base_url
            )))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }
}

/// Per-row trigger target for the backups dashboard.
#[derive(Debug, Clone, Copy)]
pub enum BackupTriggerTarget {
    Master,
    Agent(u64),
}

impl ApiClient {
    pub async fn list_webhooks(&self) -> anyhow::Result<WebhookListResponse> {
        let resp = self
            .auth(
                self.client
                    .get(format!("{}/api/v1/webhooks", self.base_url)),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn webhook_invocations(
        &self,
        service: &str,
    ) -> anyhow::Result<WebhookInvocationsResponse> {
        let resp = self
            .auth(self.client.get(format!(
                "{}/api/v1/webhooks/{service}/invocations",
                self.base_url
            )))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn add_webhook(&self, body: serde_json::Value) -> anyhow::Result<()> {
        self.auth(
            self.client
                .post(format!("{}/api/v1/webhooks", self.base_url))
                .json(&body),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn remove_webhook(&self, service: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .delete(format!("{}/api/v1/webhooks/{service}", self.base_url)),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    /// Fetch alert conversations. Returns `Ok(None)` when the server has no
    /// `[ai]` configured (HTTP 503) so the caller can render a friendly
    /// "not configured" state instead of treating it as an error.
    pub async fn alerts_list(&self, all: bool) -> anyhow::Result<Option<Vec<AlertConversation>>> {
        let url = format!("{}/api/v1/alerts?all={}", self.base_url, all);
        let resp = self.auth(self.client.get(&url)).send().await?;
        if resp.status().as_u16() == 503 {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        #[derive(Deserialize)]
        struct ListResp {
            alerts: Vec<AlertConversation>,
        }
        let body: ListResp = resp.json().await?;
        Ok(Some(body.alerts))
    }

    pub async fn alerts_reply(&self, id: &str, message: &str) -> anyhow::Result<AlertConversation> {
        let url = format!("{}/api/v1/alerts/{}/reply", self.base_url, id);
        let body = serde_json::json!({ "message": message });
        let resp = self
            .auth(self.client.post(&url))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn alerts_dismiss(&self, id: &str) -> anyhow::Result<AlertConversation> {
        let url = format!("{}/api/v1/alerts/{}/dismiss", self.base_url, id);
        let resp = self
            .auth(self.client.post(&url))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn alerts_resolve(&self, id: &str) -> anyhow::Result<AlertConversation> {
        let url = format!("{}/api/v1/alerts/{}/resolve", self.base_url, id);
        let resp = self
            .auth(self.client.post(&url))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }
}
