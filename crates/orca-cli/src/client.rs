//! HTTP client for communicating with the orca API server.

use orca_core::api_types::{
    DeployRequest, DeployResponse, ScaleRequest, ScaleResponse, StatusResponse,
};
use orca_core::config::ServicesConfig;

/// Client for the orca control plane API.
pub struct OrcaClient {
    base_url: String,
    client: reqwest::Client,
    token: Option<String>,
}

impl OrcaClient {
    /// Create a new client. Auto-reads token from `~/.orca/cluster.token` or `ORCA_TOKEN`.
    pub fn new(base_url: String) -> Self {
        let token = crate::handlers::server::read_token(None);
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

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    pub async fn deploy(&self, config: &ServicesConfig) -> anyhow::Result<DeployResponse> {
        let req = DeployRequest {
            services: config.service.clone(),
        };
        let resp = self
            .auth(self.client.post(self.url("/api/v1/deploy")))
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

    pub async fn status(&self) -> anyhow::Result<StatusResponse> {
        let resp = self
            .auth(self.client.get(self.url("/api/v1/status")))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn logs(&self, service: &str, tail: u64) -> anyhow::Result<String> {
        let resp = self
            .auth(self.client.get(self.url(&format!(
                "/api/v1/services/{service}/logs?tail={tail}&follow=false"
            ))))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    pub async fn scale(&self, service: &str, replicas: u32) -> anyhow::Result<ScaleResponse> {
        let req = ScaleRequest { replicas };
        let resp = self
            .auth(
                self.client
                    .post(self.url(&format!("/api/v1/services/{service}/scale"))),
            )
            .json(&req)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn stop(&self, service: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .delete(self.url(&format!("/api/v1/services/{service}"))),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn rollback(&self, service: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .post(self.url(&format!("/api/v1/services/{service}/rollback"))),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn promote(&self, service: &str) -> anyhow::Result<()> {
        self.auth(
            self.client
                .post(self.url(&format!("/api/v1/services/{service}/promote"))),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn stop_all(&self) -> anyhow::Result<()> {
        self.auth(self.client.post(self.url("/api/v1/stop")))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn add_webhook(
        &self,
        repo: &str,
        service: &str,
        branch: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let body = serde_json::json!({
            "repo": repo,
            "service_name": service,
            "branch": branch,
        });
        let resp = self
            .auth(self.client.post(self.url("/api/v1/webhooks")))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn list_webhooks(&self) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .auth(self.client.get(self.url("/api/v1/webhooks")))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn remove_webhook(&self, id: &str) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .auth(
                self.client
                    .delete(self.url(&format!("/api/v1/webhooks/{id}"))),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }
}
