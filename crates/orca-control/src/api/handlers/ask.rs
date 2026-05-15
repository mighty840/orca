//! `POST /api/v1/ask` — server-mediated chat with the AI.
//!
//! Lets the TUI's chat landing page (#38) drive an `orca ask`-style
//! conversation without needing local access to `cluster.toml` or the
//! secrets store. The server holds the LLM credentials and re-builds the
//! cluster context from `AppState` on every turn so answers track live
//! cluster state.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use orca_ai::ops::{ChatTurn, chat};
use orca_core::types::WorkloadStatus;

use crate::state::AppState;

#[derive(Deserialize)]
pub(crate) struct AskRequest {
    pub question: String,
    #[serde(default)]
    pub history: Vec<ChatTurn>,
}

#[derive(Serialize)]
pub(crate) struct AskResponse {
    pub response: String,
}

pub(crate) async fn ask(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AskRequest>,
) -> impl IntoResponse {
    let Some(ai) = state.cluster_config.ai.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "AI is not configured; add an [ai] block to cluster.toml"
            })),
        )
            .into_response();
    };

    if req.question.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "question must not be empty" })),
        )
            .into_response();
    }

    let status_text = render_status_text(&state).await;
    // Logs context is omitted for the chat path — the response cycle is
    // already 2-10s, and tailing every service's logs would multiply that.
    // Operators can ask "what's in the api logs?" → the LLM hints at
    // `orca logs api` and the operator runs it themselves.
    match chat(ai, &req.history, &req.question, &status_text, "").await {
        Ok(response) => Json(AskResponse { response }).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("AI request failed: {e}") })),
        )
            .into_response(),
    }
}

/// Render the live service + node state as a compact text block for the
/// LLM system prompt. Keeps the prompt budget small so we don't blow past
/// the model's context with redundant info.
async fn render_status_text(state: &AppState) -> String {
    let services = state.services.read().await;
    let nodes = state.registered_nodes.read().await;
    let mut out = String::with_capacity(1024);
    out.push_str(&format!(
        "Cluster: {}\nNodes: {}\n\n",
        state.cluster_config.cluster.name,
        nodes.len()
    ));
    out.push_str("Services:\n");
    for svc in services.values() {
        let running = svc
            .instances
            .iter()
            .filter(|i| matches!(i.status, WorkloadStatus::Running))
            .count();
        out.push_str(&format!(
            "- {} [{}/{}] desired={} ",
            svc.config.name, running, svc.desired_replicas, svc.desired_replicas
        ));
        if let Some(img) = &svc.config.image {
            out.push_str(&format!("image={img} "));
        }
        if let Some(p) = svc.config.port {
            out.push_str(&format!("port={p}"));
        }
        out.push('\n');
    }
    out
}
