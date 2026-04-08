//! HTTP handlers for the secrets store.
//!
//! Backed by `~/.orca/secrets.json` (the canonical path) so the TUI and the
//! `orca secrets ...` CLI both see the same data. Mutations are persisted
//! synchronously — the secret store writes-through on every set/remove.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use orca_core::secrets::{SecretStore, default_path};

#[derive(Debug, Deserialize)]
pub struct SetSecretRequest {
    pub value: String,
}

/// `GET /api/v1/secrets` — return the list of secret keys (never values).
pub async fn list_secrets() -> impl IntoResponse {
    match SecretStore::open(default_path()) {
        Ok(store) => {
            let keys: Vec<String> = store.list().into_iter().collect();
            (StatusCode::OK, Json(serde_json::json!({ "keys": keys }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/secrets/{key}` — set or update a secret value.
pub async fn set_secret(
    Path(key): Path<String>,
    Json(body): Json<SetSecretRequest>,
) -> impl IntoResponse {
    let mut store = match SecretStore::open(default_path()) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    match store.set(&key, &body.value) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "set", "key": key })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/secrets/{key}` — remove a secret.
pub async fn remove_secret(Path(key): Path<String>) -> impl IntoResponse {
    let mut store = match SecretStore::open(default_path()) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    match store.remove(&key) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "removed", "key": key })),
        )
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not found", "key": key })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
