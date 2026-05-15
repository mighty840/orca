//! HTTP handlers for `/api/v1/alerts/*`.
//!
//! Surface the in-memory `ConversationEngine` over JSON so the CLI/TUI can
//! list, view, reply to, dismiss, or resolve an alert conversation.
//!
//! All routes 503 when `[ai]` is unconfigured (no engine on `AppState`).
//! Reply / dismiss / resolve dispatch through `ConversationEngine` which
//! also fires the configured delivery channels.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orca_ai::context::ClusterContext;
use orca_ai::monitor::ContextProvider;
use orca_core::types::AlertConversation;

use crate::alerts::{SharedAlertEngine, StateContextProvider};
use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ListQuery {
    #[serde(default)]
    all: bool,
}

#[derive(Serialize)]
pub(crate) struct ListResponse {
    alerts: Vec<AlertConversation>,
}

#[derive(Deserialize)]
pub(crate) struct ReplyBody {
    message: String,
}

fn unconfigured() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "AI alerts are not configured; add an [ai] block to cluster.toml"
        })),
    )
}

fn not_found(id: Uuid) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": format!("alert {id} not found") })),
    )
}

fn engine(state: &Arc<AppState>) -> Option<SharedAlertEngine> {
    state.alerts.clone()
}

/// Build a fresh `ClusterContext` from `AppState`. The engine needs this on
/// every operator interaction so the LLM responds with current cluster state
/// rather than the snapshot taken when the alert was first opened.
async fn current_context(state: &Arc<AppState>) -> ClusterContext {
    let provider = StateContextProvider::for_state(state.clone());
    provider
        .snapshot()
        .await
        .unwrap_or_else(|_| ClusterContext {
            cluster_name: state.cluster_config.cluster.name.clone(),
            nodes: Vec::new(),
            services: Vec::new(),
            recent_events: Vec::new(),
            active_alerts: Vec::new(),
        })
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    let Some(engine) = engine(&state) else {
        return unconfigured().into_response();
    };
    let guard = engine.read().await;
    let alerts: Vec<AlertConversation> = if query.all {
        guard.all_conversations().to_vec()
    } else {
        guard.active_conversations().into_iter().cloned().collect()
    };
    Json(ListResponse { alerts }).into_response()
}

pub(crate) async fn view(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(engine) = engine(&state) else {
        return unconfigured().into_response();
    };
    let guard = engine.read().await;
    match guard.get_conversation(id) {
        Some(c) => Json(c.clone()).into_response(),
        None => not_found(id).into_response(),
    }
}

pub(crate) async fn reply(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<ReplyBody>,
) -> impl IntoResponse {
    let Some(engine) = engine(&state) else {
        return unconfigured().into_response();
    };
    let ctx = current_context(&state).await;
    let mut guard = engine.write().await;
    match guard.operator_reply(id, &body.message, &ctx).await {
        Ok(c) => Json(c.clone()).into_response(),
        Err(e) => {
            let msg = format!("{e}");
            if msg.contains("conversation not found") {
                not_found(id).into_response()
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": msg })),
                )
                    .into_response()
            }
        }
    }
}

pub(crate) async fn dismiss(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    inline_reply(state, id, "dismiss").await
}

pub(crate) async fn resolve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    inline_reply(state, id, "resolve").await
}

/// Issue a built-in operator message ("dismiss" / "resolve") through the
/// engine. The engine's existing special-command path translates these into
/// state transitions + channel dispatches.
async fn inline_reply(state: Arc<AppState>, id: Uuid, marker: &str) -> axum::response::Response {
    let Some(engine) = engine(&state) else {
        return unconfigured().into_response();
    };
    let ctx = current_context(&state).await;
    let mut guard = engine.write().await;
    match guard.operator_reply(id, marker, &ctx).await {
        Ok(c) => Json(c.clone()).into_response(),
        Err(e) => {
            let msg = format!("{e}");
            if msg.contains("conversation not found") {
                not_found(id).into_response()
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": msg })),
                )
                    .into_response()
            }
        }
    }
}
