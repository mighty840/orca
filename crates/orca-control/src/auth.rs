//! Bearer token authentication middleware for the API server.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Paths that skip bearer token authentication.
const SKIP_AUTH_PATHS: &[&str] = &["/api/v1/health", "/api/v1/webhooks/github"];

/// Axum middleware that validates `Authorization: Bearer <token>` headers.
///
/// If `AppState::api_tokens` is empty, all requests are allowed (backward compatible).
/// Health and webhook endpoints are always exempt.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let tokens = &state.api_tokens;

    // If no tokens configured, allow everything (backward compatible)
    if tokens.is_empty() {
        return next.run(request).await;
    }

    // Skip auth for exempt paths
    let path = request.uri().path();
    if SKIP_AUTH_PATHS.contains(&path) {
        return next.run(request).await;
    }

    // Extract and validate bearer token
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if tokens.iter().any(|t| t == token) {
                next.run(request).await
            } else {
                (StatusCode::UNAUTHORIZED, "invalid bearer token").into_response()
            }
        }
        _ => (StatusCode::UNAUTHORIZED, "missing bearer token").into_response(),
    }
}
