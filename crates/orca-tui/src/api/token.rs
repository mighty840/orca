//! Cluster-token rotation (`orca token rotate`, #210) for the TUI (#265).
//! Admin only on the server. No response ever contains a token.

use serde::Deserialize;

use super::ApiClient;

#[derive(Debug, Clone, Deserialize)]
pub struct RotationStatus {
    pub in_progress: bool,
    pub token_file: String,
    #[serde(default)]
    pub nodes: Vec<RotationNode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RotationNode {
    pub node_id: u64,
    pub address: String,
    /// It uses the new token now.
    #[serde(default)]
    pub rotated: bool,
    /// Its next start uses it too.
    #[serde(default)]
    pub persisted: bool,
    /// Where it saved the token, or what must be changed by hand.
    pub detail: Option<String>,
}

impl ApiClient {
    pub async fn token_rotation(&self) -> anyhow::Result<RotationStatus> {
        let url = format!("{}/api/v1/cluster/token/rotation", self.base_url);
        rotation_reply(self.auth(self.client.get(url)).send().await?).await
    }

    /// Start a rotation: the master writes a new token and keeps accepting
    /// the old one until [`Self::finish_token_rotation`].
    pub async fn start_token_rotation(&self) -> anyhow::Result<RotationStatus> {
        let url = format!("{}/api/v1/cluster/token/rotate", self.base_url);
        rotation_reply(self.auth(self.client.post(url)).send().await?).await
    }

    /// Retire the old token. Refused (409) while an agent isn't on the new
    /// one for good, unless `force`.
    pub async fn finish_token_rotation(&self, force: bool) -> anyhow::Result<RotationStatus> {
        let url = format!("{}/api/v1/cluster/token/rotation/finish", self.base_url);
        let body = serde_json::json!({ "force": force });
        rotation_reply(self.auth(self.client.post(url)).json(&body).send().await?).await
    }
}

/// The server explains a refusal in the body (409), so surface that text
/// rather than only the status code.
async fn rotation_reply(resp: reqwest::Response) -> anyhow::Result<RotationStatus> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp.json().await?);
    }
    let text = resp.text().await.unwrap_or_default();
    match status.as_u16() {
        403 => anyhow::bail!("token rotation needs an admin token"),
        _ if !text.trim().is_empty() => anyhow::bail!("{}", text.trim()),
        _ => anyhow::bail!("HTTP {status}"),
    }
}
