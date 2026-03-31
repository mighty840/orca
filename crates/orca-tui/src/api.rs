//! API client for fetching cluster data from the orca control plane.

use serde::Deserialize;

/// Fetches cluster data from the orca API.
pub struct ApiClient {
    base_url: String,
    client: reqwest::Client,
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
    pub runtime: String,
    pub desired_replicas: u32,
    pub running_replicas: u32,
    pub status: String,
    pub domain: Option<String>,
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
}

impl ApiClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn status(&self) -> anyhow::Result<StatusResponse> {
        let resp = self
            .client
            .get(format!("{}/api/v1/status", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn cluster_info(&self) -> anyhow::Result<ClusterInfo> {
        let resp = self
            .client
            .get(format!("{}/api/v1/cluster/info", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn logs(&self, service: &str, tail: u64) -> anyhow::Result<String> {
        let resp = self
            .client
            .get(format!(
                "{}/api/v1/services/{service}/logs?tail={tail}&follow=false",
                self.base_url
            ))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    /// Trigger a redeploy for a service.
    pub async fn deploy(&self, service: &str) -> anyhow::Result<()> {
        self.client
            .post(format!(
                "{}/api/v1/services/{service}/deploy",
                self.base_url
            ))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Stop a service (scale to 0).
    pub async fn stop(&self, service: &str) -> anyhow::Result<()> {
        self.client
            .post(format!("{}/api/v1/services/{service}/stop", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}
