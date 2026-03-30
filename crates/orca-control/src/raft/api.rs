//! Axum routes for incoming Raft RPCs.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};

use super::OrcaRaft;

/// Shared Raft handle for axum handlers.
pub type RaftState = Arc<OrcaRaft>;

/// Build an axum `Router` for Raft RPC endpoints.
///
/// Mount this under `/raft` in the main API router.
pub fn raft_router() -> Router<RaftState> {
    Router::new()
        .route("/raft/append", post(handle_append))
        .route("/raft/vote", post(handle_vote))
        .route("/raft/snapshot", post(handle_snapshot))
}

async fn handle_append(
    State(raft): State<RaftState>,
    Json(rpc): Json<AppendEntriesRequest<super::type_config::OrcaTypeConfig>>,
) -> Result<Json<AppendEntriesResponse<u64>>, StatusCode> {
    raft.append_entries(rpc)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn handle_vote(
    State(raft): State<RaftState>,
    Json(rpc): Json<VoteRequest<u64>>,
) -> Result<Json<VoteResponse<u64>>, StatusCode> {
    raft.vote(rpc)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn handle_snapshot(
    State(raft): State<RaftState>,
    Json(rpc): Json<InstallSnapshotRequest<super::type_config::OrcaTypeConfig>>,
) -> Result<Json<InstallSnapshotResponse<u64>>, StatusCode> {
    raft.install_snapshot(rpc)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
