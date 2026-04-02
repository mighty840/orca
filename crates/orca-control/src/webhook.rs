//! Webhook handler for GitHub/Gitea/GitLab push events.
//!
//! When a push webhook fires, orca looks up the matching service and triggers
//! a rolling redeploy (stop all instances, pull fresh image, recreate).

use std::sync::Arc;

use axum::extract::Path;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, post};
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::reconciler;
use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

/// Configuration for a webhook trigger.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookConfig {
    /// Repository full name, e.g. "myorg/api".
    pub repo: String,
    /// Orca service name to redeploy.
    pub service_name: String,
    /// Branch to watch (default: "main").
    #[serde(default = "default_branch")]
    pub branch: String,
    /// Optional HMAC secret for signature validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

fn default_branch() -> String {
    "main".to_string()
}

/// Shared webhook config store, stored in [`AppState`] extension.
pub type WebhookStore = Arc<RwLock<Vec<WebhookConfig>>>;

/// Create a new empty webhook store.
pub fn new_store() -> WebhookStore {
    Arc::new(RwLock::new(Vec::new()))
}

/// Subset of GitHub push webhook payload we care about.
#[derive(Debug, serde::Deserialize)]
struct PushPayload {
    /// e.g. "refs/heads/main"
    #[serde(rename = "ref")]
    git_ref: String,
    repository: RepoInfo,
    head_commit: Option<CommitInfo>,
}

#[derive(Debug, serde::Deserialize)]
struct RepoInfo {
    full_name: String,
}

#[derive(Debug, serde::Deserialize)]
struct CommitInfo {
    id: String,
    message: String,
}

/// Extract branch name from a git ref like "refs/heads/main".
fn branch_from_ref(git_ref: &str) -> Option<&str> {
    git_ref.strip_prefix("refs/heads/")
}

/// Validate HMAC-SHA256 signature from the `X-Hub-Signature-256` header.
fn validate_signature(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let Some(hex_sig) = signature_header.strip_prefix("sha256=") else {
        return false;
    };

    let Ok(expected) = hex::decode(hex_sig) else {
        return false;
    };

    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };

    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// Handle a GitHub/Gitea push webhook.
///
/// Mounted at `POST /api/v1/webhooks/github`.
/// Build webhook routes.
/// Build webhook routes (call before with_state on parent router).
pub fn webhook_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/webhooks/github", post(handle_push))
        .route("/api/v1/webhooks", post(register).get(list))
        .route("/api/v1/webhooks/{id}", delete(remove_webhook))
}

pub async fn handle_push(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // Parse the payload
    let payload: PushPayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            warn!("Webhook: invalid payload: {e}");
            return (StatusCode::BAD_REQUEST, format!("invalid payload: {e}")).into_response();
        }
    };

    let repo = &payload.repository.full_name;
    let Some(branch) = branch_from_ref(&payload.git_ref) else {
        return (StatusCode::OK, "ignored: not a branch push".to_string()).into_response();
    };

    let commit_id = payload
        .head_commit
        .as_ref()
        .map(|c| c.id.as_str())
        .unwrap_or("unknown");
    let commit_msg = payload
        .head_commit
        .as_ref()
        .and_then(|c| c.message.lines().next())
        .unwrap_or("");
    let short_sha = &commit_id[..commit_id.len().min(8)];

    info!("Webhook: push to {repo}#{branch} (commit {short_sha}: {commit_msg})");

    // Find matching webhook config
    let webhooks = state.webhooks.read().await;
    let matching: Vec<WebhookConfig> = webhooks
        .iter()
        .filter(|w| w.repo == *repo && w.branch == branch)
        .cloned()
        .collect();
    drop(webhooks);

    if matching.is_empty() {
        info!("Webhook: no config for {repo}#{branch}, ignoring");
        return (
            StatusCode::OK,
            "ignored: no matching webhook config".to_string(),
        )
            .into_response();
    }

    let sig_header = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let mut deployed = Vec::new();
    let mut errors = Vec::new();
    let mut sig_failures = 0u32;

    for wh in &matching {
        // Validate secret if configured
        if let Some(secret) = &wh.secret
            && (sig_header.is_empty() || !validate_signature(secret, &body, sig_header))
        {
            sig_failures += 1;
            warn!("Webhook: HMAC validation failed for {}", wh.service_name);
            continue;
        }

        info!("Webhook: triggering redeploy of {}", wh.service_name);
        match reconciler::redeploy(&state, &wh.service_name).await {
            Ok(()) => deployed.push(wh.service_name.clone()),
            Err(e) => {
                error!("Webhook: redeploy of {} failed: {e}", wh.service_name);
                errors.push(format!("{}: {e}", wh.service_name));
            }
        }
    }

    // If every matching webhook failed signature validation, return 401
    if sig_failures > 0 && deployed.is_empty() && errors.is_empty() {
        return (StatusCode::UNAUTHORIZED, "signature validation failed").into_response();
    }

    let status = if errors.is_empty() {
        StatusCode::OK
    } else if deployed.is_empty() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::PARTIAL_CONTENT
    };

    (
        status,
        Json(serde_json::json!({ "deployed": deployed, "errors": errors })),
    )
        .into_response()
}

/// Register a new webhook config.
///
/// Mounted at `POST /api/v1/webhooks`.
pub async fn register(
    State(state): State<Arc<AppState>>,
    Json(config): Json<WebhookConfig>,
) -> impl IntoResponse {
    info!(
        "Webhook: registering {}#{} -> {}",
        config.repo, config.branch, config.service_name
    );
    let mut webhooks = state.webhooks.write().await;
    // Remove existing config for same repo+branch+service to allow updates
    webhooks.retain(|w| {
        !(w.repo == config.repo
            && w.branch == config.branch
            && w.service_name == config.service_name)
    });
    webhooks.push(config);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"status": "registered"})),
    )
}

/// List all webhook configs.
///
/// Mounted at `GET /api/v1/webhooks`.
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let webhooks = state.webhooks.read().await;
    Json(serde_json::json!({ "webhooks": *webhooks }))
}

/// Remove a webhook by service name.
///
/// Mounted at `DELETE /api/v1/webhooks/{id}` where id is the service_name.
pub async fn remove_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut webhooks = state.webhooks.write().await;
    let before = webhooks.len();
    webhooks.retain(|w| w.service_name != id);
    let removed = before - webhooks.len();

    if removed == 0 {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("no webhook for service '{id}'")})),
        )
            .into_response()
    } else {
        info!("Webhook: removed {removed} webhook(s) for service '{id}'");
        (
            StatusCode::OK,
            Json(serde_json::json!({"status": "removed", "count": removed})),
        )
            .into_response()
    }
}

#[cfg(test)]
#[path = "webhook_tests.rs"]
mod tests;
