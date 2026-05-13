//! Webhook invocation tracking — per-service ring buffer of recent pushes,
//! plus the `GET /api/v1/webhooks/{id}/invocations` handler that exposes it.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;

use orca_core::api_types::{WebhookInvocation, WebhookInvocationsResponse};

use crate::state::AppState;
use crate::webhook::WebhookConfig;

/// Recent-invocation ring buffer length, per service.
pub(crate) const INVOCATION_HISTORY_LEN: usize = 10;

/// Return the recent-invocation ring buffer for one webhook.
///
/// Mounted at `GET /api/v1/webhooks/{id}/invocations` where `id` is the
/// service_name.
pub async fn invocations(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let map = state.webhook_invocations.read().await;
    let list = map
        .get(&id)
        .map(|q| q.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    Json(WebhookInvocationsResponse { invocations: list })
}

/// Append an invocation to the per-service ring buffer, evicting the oldest
/// entry when the cap is reached. Keyed by `service_name` to match how the
/// webhook is identified everywhere else (delete endpoint, list response).
pub(crate) async fn record_invocation(
    state: &AppState,
    wh: &WebhookConfig,
    commit_sha: &str,
    status_code: u16,
    deployed: bool,
) {
    let inv = WebhookInvocation {
        at: chrono::Utc::now(),
        status_code,
        commit_sha: commit_sha.to_string(),
        repo: wh.repo.clone(),
        branch: wh.branch.clone(),
        deployed,
    };
    let mut map = state.webhook_invocations.write().await;
    let ring = map.entry(wh.service_name.clone()).or_default();
    if ring.len() >= INVOCATION_HISTORY_LEN {
        ring.pop_front();
    }
    ring.push_back(inv);
}
