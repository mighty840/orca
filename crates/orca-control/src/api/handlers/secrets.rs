//! HTTP handlers for the secrets store.
//!
//! Backed by `~/.orca/secrets.json` (the canonical path) so the TUI and the
//! `orca secrets ...` CLI both see the same data. Mutations are persisted
//! synchronously — the secret store writes-through on every set/remove.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use orca_core::api_types::{SecretRef, SecretUsage, SecretsUsageResponse};
use orca_core::secrets::extract_refs;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SetSecretRequest {
    pub value: String,
}

/// `GET /api/v1/secrets` — return the list of secret keys (never values).
pub async fn list_secrets() -> impl IntoResponse {
    match orca_core::secrets::open_configured() {
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
    let mut store = match orca_core::secrets::open_configured() {
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

/// `GET /api/v1/secrets/usage` — return every secret key in the store
/// alongside the services that reference it. Used by the TUI's secrets
/// organizer view. Also includes "orphan refs" — services that template a
/// key that isn't in the store — so the operator sees broken templates,
/// not just stored keys.
pub async fn secrets_usage(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let store = match orca_core::secrets::open_configured() {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let stored_keys: Vec<String> = store.list();

    // Build key → unique (service, project) set. Using BTreeMap so the
    // response order is stable for the dashboard.
    let mut refs_by_key: BTreeMap<(Option<String>, String), Vec<SecretRef>> = BTreeMap::new();
    {
        let services = state.services.read().await;
        for svc in services.values() {
            let project = svc.config.project.clone();
            let mut seen_for_service: std::collections::HashSet<(Option<String>, String)> =
                std::collections::HashSet::new();
            for value in svc.config.env.values() {
                for r in extract_refs(value) {
                    let map_key = (r.scope.clone(), r.key.clone());
                    if seen_for_service.insert(map_key.clone()) {
                        refs_by_key.entry(map_key).or_default().push(SecretRef {
                            service_name: svc.config.name.clone(),
                            project: project.clone(),
                        });
                    }
                }
            }
        }
    }

    // Emit stored keys first (preserving alphabetical order from the store),
    // then any referenced-but-unstored keys as "broken" entries.
    let mut out: Vec<SecretUsage> = stored_keys
        .iter()
        .map(|k| {
            let map_key = (None, k.clone());
            let refs = refs_by_key.remove(&map_key).unwrap_or_default();
            SecretUsage {
                key: k.clone(),
                scope: None,
                refs,
                in_store: true,
            }
        })
        .collect();
    for ((scope, key), refs) in refs_by_key {
        out.push(SecretUsage {
            key,
            scope,
            refs,
            in_store: false,
        });
    }

    Json(SecretsUsageResponse { secrets: out }).into_response()
}

/// `DELETE /api/v1/secrets/{key}` — remove a secret.
pub async fn remove_secret(Path(key): Path<String>) -> impl IntoResponse {
    let mut store = match orca_core::secrets::open_configured() {
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
