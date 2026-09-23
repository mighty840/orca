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
use tracing::{error, info, warn};

use crate::operations::AgentOfflineError;
use crate::reconciler;
use crate::state::AppState;
use crate::webhook_auth::{SECRET_REQUIRED, short_sha, validate_signature};
use crate::webhook_invocations::record_invocation;

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
    /// If true, this is an infra webhook: git pull + redeploy all services.
    #[serde(default)]
    pub infra: bool,
}

fn default_branch() -> String {
    "main".to_string()
}

use crate::webhook_store::persist;
pub use crate::webhook_store::{WebhookStore, new_store};

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

/// Handle a GitHub/Gitea push webhook.
///
/// Mounted at `POST /api/v1/webhooks/github`.
/// Build webhook routes.
/// Build webhook routes (call before with_state on parent router).
pub fn webhook_router() -> Router<Arc<AppState>> {
    use axum::routing::get;
    Router::new()
        .route("/api/v1/webhooks/github", post(handle_push))
        .route("/api/v1/webhooks", post(register).get(list))
        .route("/api/v1/webhooks/{id}", delete(remove_webhook))
        .route(
            "/api/v1/webhooks/{id}/invocations",
            get(crate::webhook_invocations::invocations),
        )
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
    let short_sha = short_sha(commit_id);

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
    let mut agent_offline = false;

    for wh in &matching {
        // Fail closed. A webhook without a secret cannot authenticate a push,
        // so it must never deploy. The API refuses to register one, but a
        // legacy entry in webhooks.json can still carry `secret: None`.
        let authenticated = match wh.effective_secret() {
            Some(secret) => !sig_header.is_empty() && validate_signature(secret, &body, sig_header),
            None => {
                error!(
                    "Webhook: {} has no secret configured; rejecting push. \
                     Re-register it with `orca webhooks add`.",
                    wh.service_name
                );
                false
            }
        };
        if !authenticated {
            sig_failures += 1;
            warn!("Webhook: HMAC validation failed for {}", wh.service_name);
            record_invocation(&state, wh, short_sha, 401, false).await;
            continue;
        }

        if wh.infra {
            info!("Webhook: infra push detected, spawning git pull + deploy all");
            let state_clone = Arc::clone(&state);
            tokio::spawn(async move {
                match handle_infra_deploy(&state_clone).await {
                    Ok(count) => info!("Infra deploy complete: {count} services"),
                    Err(e) => error!("Webhook: infra deploy failed: {e}"),
                }
            });
            deployed.push("infra (deploying)".to_string());
            record_invocation(&state, wh, short_sha, 202, true).await;
            continue;
        }

        info!("Webhook: triggering redeploy of {}", wh.service_name);
        match reconciler::redeploy(&state, &wh.service_name).await {
            Ok(()) => {
                deployed.push(wh.service_name.clone());
                record_invocation(&state, wh, short_sha, 200, true).await;
            }
            Err(e) => {
                let offline = e.downcast_ref::<AgentOfflineError>().is_some();
                if offline {
                    agent_offline = true;
                }
                error!("Webhook: redeploy of {} failed: {e}", wh.service_name);
                errors.push(format!("{}: {e}", wh.service_name));
                let code = if offline { 503 } else { 500 };
                record_invocation(&state, wh, short_sha, code, false).await;
            }
        }
    }

    // If every matching webhook failed signature validation, return 401
    if sig_failures > 0 && deployed.is_empty() && errors.is_empty() {
        return (StatusCode::UNAUTHORIZED, "signature validation failed").into_response();
    }

    let status = if errors.is_empty() {
        StatusCode::OK
    } else if deployed.is_empty() && agent_offline {
        StatusCode::SERVICE_UNAVAILABLE
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
    if config.effective_secret().is_none() {
        warn!(
            "Webhook: refusing to register {}#{} -> {} without a secret",
            config.repo, config.branch, config.service_name
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": SECRET_REQUIRED })),
        )
            .into_response();
    }
    info!(
        "Webhook: registering {}#{} -> {}",
        config.repo, config.branch, config.service_name
    );
    {
        let mut webhooks = state.webhooks.write().await;
        // Remove existing config for same repo+branch+service to allow updates
        webhooks.retain(|w| {
            !(w.repo == config.repo
                && w.branch == config.branch
                && w.service_name == config.service_name)
        });
        webhooks.push(config);
    }
    persist(&state.webhooks).await;
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"status": "registered"})),
    )
        .into_response()
}

/// List all webhook configs.
///
/// Mounted at `GET /api/v1/webhooks`. Each entry carries the most recent
/// invocation (if any) so the TUI dashboard renders without an N+1 fetch.
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use orca_core::api_types::{WebhookEntry, WebhookListResponse};
    let webhooks = state.webhooks.read().await;
    let invocations = state.webhook_invocations.read().await;
    let entries: Vec<WebhookEntry> = webhooks
        .iter()
        .map(|w| WebhookEntry {
            repo: w.repo.clone(),
            service_name: w.service_name.clone(),
            branch: w.branch.clone(),
            has_secret: w.effective_secret().is_some(),
            infra: w.infra,
            last_invocation: invocations
                .get(&w.service_name)
                .and_then(|q| q.back())
                .cloned(),
        })
        .collect();
    Json(WebhookListResponse { webhooks: entries })
}

/// Remove a webhook by service name.
///
/// Mounted at `DELETE /api/v1/webhooks/{id}` where id is the service_name.
pub async fn remove_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let removed = {
        let mut webhooks = state.webhooks.write().await;
        let before = webhooks.len();
        webhooks.retain(|w| w.service_name != id);
        before - webhooks.len()
    };
    if removed > 0 {
        persist(&state.webhooks).await;
    }

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

/// Handle an infra webhook: git pull the working directory, then redeploy
/// all services from the refreshed service.toml files.
async fn handle_infra_deploy(state: &AppState) -> anyhow::Result<usize> {
    // Run `git pull` in the current working directory
    let output = tokio::process::Command::new("git")
        .args(["pull", "--ff-only"])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git pull failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    info!("Infra git pull: {}", stdout.trim());

    // Load all services from the services/ directory
    let services_dir = std::path::Path::new("services");
    let configs = if services_dir.is_dir() {
        orca_core::config::ServicesConfig::load_dir(services_dir)?
    } else {
        orca_core::config::ServicesConfig::load("services.toml".as_ref())?
    };

    // Secrets are resolved in service_config_to_spec() at container creation
    // time, not here. This ensures spec_matches() compares unresolved templates
    // and doesn't restart containers just because a token was refreshed.
    //
    // Reconcile only NEW or SPEC-CHANGED services (#120) — same filter the
    // declarative loop applies. Reconciling the full tree on every infra
    // push recreated every placement-pinned service across unrelated
    // projects (the 2026-07-07 incident); a push touching one file must
    // only converge the services it actually changed.
    let changed: Vec<orca_core::config::ServiceConfig> = {
        let services = state.services.read().await;
        configs
            .service
            .iter()
            .filter(|cfg| match services.get(&cfg.name) {
                None => true,
                Some(svc) => !svc.config.spec_matches(cfg),
            })
            .cloned()
            .collect()
    };
    let count = changed.len();
    let (deployed, errors) = reconciler::reconcile(state, &changed).await;

    // Persist deployed services to store
    if let Some(store) = &state.store {
        for config in &changed {
            if deployed.contains(&config.name)
                && let Err(e) = store.set_service(&config.name, config)
            {
                tracing::warn!("Failed to persist {}: {e}", config.name);
            }
        }
    }

    if !errors.is_empty() {
        warn!("Infra deploy: {} errors: {:?}", errors.len(), errors);
    }
    info!(
        "Infra deploy complete: {}/{} services",
        deployed.len(),
        count
    );
    Ok(count)
}

#[cfg(test)]
#[path = "webhook_tests.rs"]
mod tests;
