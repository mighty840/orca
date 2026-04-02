//! API client for fetching cluster data from the orca control plane.

use std::collections::HashMap;

use serde::Deserialize;

/// Fetches cluster data from the orca API.
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
    pub project: Option<String>,
    #[serde(default)]
    pub memory_usage: Option<String>,
    #[serde(default)]
    pub cpu_percent: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ClusterInfo {
    pub cluster_name: String,
    pub node_count: u64,
    pub nodes: Vec<NodeInfo>,
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
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
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
}
