//! HTTP WebSocket handler for interactive exec sessions.
//!
//! Bridges the CLI WS connection to the agent WS channel using session_id
//! multiplexing. Local containers are exec'd directly via the container runtime.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use base64::Engine as _;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use orca_core::ws_types::MasterMessage;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct ExecQuery {
    #[serde(default = "default_cmd")]
    pub cmd: String,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

fn default_cmd() -> String {
    "/bin/sh".into()
}
fn default_cols() -> u16 {
    80
}
fn default_rows() -> u16 {
    24
}

pub async fn exec_ws_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(params): Query<ExecQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_exec_ws(socket, state, name, params))
}

async fn handle_exec_ws(
    mut socket: WebSocket,
    state: Arc<AppState>,
    service_name: String,
    params: ExecQuery,
) {
    let cmd: Vec<String> = params.cmd.split(',').map(|s| s.to_string()).collect();

    let node_id = find_remote_node_id(&state, &service_name).await;

    if let Some(node_id) = node_id {
        run_remote_exec(
            socket,
            state,
            service_name,
            cmd,
            node_id,
            params.cols,
            params.rows,
        )
        .await;
    } else {
        socket
            .send(Message::Text(
                "local exec via WS not yet supported — use orca exec directly".into(),
            ))
            .await
            .ok();
    }
}

async fn run_remote_exec(
    mut socket: WebSocket,
    state: Arc<AppState>,
    service_name: String,
    cmd: Vec<String>,
    node_id: u64,
    cols: u16,
    rows: u16,
) {
    let session_id = Uuid::new_v4().to_string();
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(256);

    state
        .exec_sessions
        .write()
        .await
        .insert(session_id.clone(), output_tx);

    let sent = {
        let agents = state.ws_agents.read().await;
        if let Some(agent_tx) = agents.get(&node_id) {
            agent_tx
                .send(MasterMessage::ExecStart {
                    session_id: session_id.clone(),
                    service_name,
                    cmd,
                    cols,
                    rows,
                })
                .await
                .is_ok()
        } else {
            false
        }
    };

    if !sent {
        error!("exec: agent {node_id} not connected");
        state.exec_sessions.write().await.remove(&session_id);
        return;
    }

    info!("exec: session {session_id} started for node {node_id}");

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                        let agents = state.ws_agents.read().await;
                        if let Some(agent_tx) = agents.get(&node_id) {
                            let _ = agent_tx
                                .send(MasterMessage::ExecInput {
                                    session_id: session_id.clone(),
                                    data: encoded,
                                })
                                .await;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(resize) = serde_json::from_str::<ResizeEvent>(&text) {
                            let agents = state.ws_agents.read().await;
                            if let Some(agent_tx) = agents.get(&node_id) {
                                let _ = agent_tx
                                    .send(MasterMessage::ExecResize {
                                        session_id: session_id.clone(),
                                        cols: resize.cols,
                                        rows: resize.rows,
                                    })
                                    .await;
                            }
                        }
                    }
                    None | Some(Ok(Message::Close(_))) => break,
                    Some(Err(e)) => {
                        warn!("exec: socket error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
            Some(data) = output_rx.recv() => {
                if socket.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
            else => break,
        }
    }

    let agents = state.ws_agents.read().await;
    if let Some(agent_tx) = agents.get(&node_id) {
        let _ = agent_tx
            .send(MasterMessage::ExecClose {
                session_id: session_id.clone(),
            })
            .await;
    }
    state.exec_sessions.write().await.remove(&session_id);
    info!("exec: session {session_id} closed");
}

async fn find_remote_node_id(state: &AppState, service_name: &str) -> Option<u64> {
    let services = state.services.read().await;
    let svc = services.get(service_name)?;
    svc.instances.iter().find_map(|i| {
        i.handle
            .runtime_id
            .strip_prefix("remote-")
            .and_then(|s| s.parse::<u64>().ok())
    })
}

#[derive(serde::Deserialize)]
struct ResizeEvent {
    cols: u16,
    rows: u16,
}
