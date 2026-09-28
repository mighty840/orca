//! Webhooks and AI alert conversations.

use serde::Deserialize;

use super::{AlertConversation, ApiClient, WebhookInvocationsResponse, WebhookListResponse};

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
