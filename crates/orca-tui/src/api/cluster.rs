//! Cluster-wide dashboards: networks, secrets and backups.

use super::{
    ApiClient, ClusterBackupsResponse, ClusterNetworksResponse, SecretListResponse,
    SecretsUsageResponse, TriggerBackupResponse,
};

impl ApiClient {
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
