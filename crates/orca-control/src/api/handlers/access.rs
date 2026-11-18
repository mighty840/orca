//! `GET /api/v1/whoami` and `GET /api/v1/audit` (#80).

use axum::Json;
use axum::extract::{Extension, Query, State};
use orca_core::api_types::{AuditEvent, WhoAmI};
use serde::Deserialize;
use std::sync::Arc;

use crate::auth::Principal;
use crate::state::AppState;

/// The caller's token name, role and allowed actions. Lets a client hide
/// what it may not do, and lets MCP refuse the cluster token.
pub(crate) async fn whoami(Extension(p): Extension<Principal>) -> Json<WhoAmI> {
    Json(WhoAmI {
        actions: p.actions(),
        name: p.name,
        role: p.role,
        legacy: p.legacy,
        open: p.open,
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuditQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

/// The newest audited calls, newest first.
pub(crate) async fn audit(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuditQuery>,
) -> Json<Vec<AuditEvent>> {
    Json(state.audit.recent(q.limit.min(crate::audit::CAPACITY)))
}
