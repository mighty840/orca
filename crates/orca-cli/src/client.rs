//! HTTP client for communicating with the orca API server.

use orca_core::api_types::{
    DeployRequest, DeployResponse, ScaleRequest, ScaleResponse, StatusResponse,
};
use orca_core::config::ServicesConfig;

/// Client for the orca control plane API.
pub struct OrcaClient {
    base_url: String,
    client: reqwest::Client,
}

impl OrcaClient {
    /// Create a new client pointing at the given API base URL.
    pub fn new(base_url: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Deploy services to the cluster.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails.
    pub async fn deploy(&self, config: &ServicesConfig) -> anyhow::Result<DeployResponse> {
        let req = DeployRequest {
            services: config.service.clone(),
        };
        let resp = self
            .client
            .post(format!("{}/api/v1/deploy", self.base_url))
            .json(&req)
            .send()
            .await?;

        if !resp.status().is_success() && resp.status().as_u16() != 206 {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("deploy failed (HTTP {status}): {body}");
        }

        Ok(resp.json().await?)
    }

    /// Get cluster and service status.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails.
    pub async fn status(&self) -> anyhow::Result<StatusResponse> {
        let resp = self
            .client
            .get(format!("{}/api/v1/status", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Get logs for a service.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or the service is not found.
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

    /// Scale a service to the given replica count.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails.
    pub async fn scale(&self, service: &str, replicas: u32) -> anyhow::Result<ScaleResponse> {
        let req = ScaleRequest { replicas };
        let resp = self
            .client
            .post(format!("{}/api/v1/services/{service}/scale", self.base_url))
            .json(&req)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Stop a specific service.
    pub async fn stop(&self, service: &str) -> anyhow::Result<()> {
        self.client
            .delete(format!("{}/api/v1/services/{service}", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Rollback a service to its previous deploy.
    pub async fn rollback(&self, service: &str) -> anyhow::Result<()> {
        self.client
            .post(format!(
                "{}/api/v1/services/{service}/rollback",
                self.base_url
            ))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Stop all services.
    pub async fn stop_all(&self) -> anyhow::Result<()> {
        self.client
            .post(format!("{}/api/v1/stop", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}
