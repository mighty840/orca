//! `orca token rotate` endpoints (#210). Admin only; none returns a token.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::state::AppState;
use crate::token_rotation::{self, RotationStatus};

fn reply(result: anyhow::Result<RotationStatus>) -> Response {
    match result {
        Ok(status) => Json(status).into_response(),
        Err(e) => (StatusCode::CONFLICT, e.to_string()).into_response(),
    }
}

/// POST /api/v1/cluster/token/rotate
pub(crate) async fn start(State(state): State<Arc<AppState>>) -> Response {
    reply(token_rotation::start(&state, &token_rotation::token_dir()).await)
}

/// GET /api/v1/cluster/token/rotation
pub(crate) async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(token_rotation::status(&state, &token_rotation::token_dir()).await)
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FinishRequest {
    #[serde(default)]
    force: bool,
}

/// POST /api/v1/cluster/token/rotation/finish
pub(crate) async fn finish(
    State(state): State<Arc<AppState>>,
    body: Option<Json<FinishRequest>>,
) -> Response {
    let force = body.map(|Json(b)| b.force).unwrap_or_default();
    reply(token_rotation::finish(&state, &token_rotation::token_dir(), force).await)
}
