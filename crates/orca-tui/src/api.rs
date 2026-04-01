//! API client for fetching cluster data from the orca control plane.

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
        // Read token from ~/.orca/cluster.token or ORCA_TOKEN env
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

    pub async fn deploy(&self, service: &str) -> anyhow::Result<()> {
        self.auth(self.client.post(format!(
            "{}/api/v1/services/{service}/deploy",
            self.base_url
        )))
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn stop(&self, service: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .post(format!("{}/api/v1/services/{service}/stop", self.base_url)),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }
}
