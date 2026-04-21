//! Interactive PTY exec session handling for agent nodes.
//!
//! Each exec session is multiplexed over the existing agent WS channel using
//! a session_id. The agent creates a Docker exec with PTY, then pumps output
//! via AgentMessage::ExecOutput and receives stdin via ExecInput.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use bollard::Docker;
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecResults};
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::{RwLock, mpsc};
use tracing::{error, info, warn};

use orca_core::ws_types::AgentMessage;

pub type ExecSessions = Arc<RwLock<HashMap<String, mpsc::Sender<Vec<u8>>>>>;

/// Start a new interactive PTY exec session on a container.
pub async fn start_exec(
    session_id: String,
    service_name: String,
    cmd: Vec<String>,
    cols: u16,
    rows: u16,
    sessions: ExecSessions,
    out_tx: mpsc::Sender<AgentMessage>,
) {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            error!("exec: docker connect failed: {e}");
            send_done(&out_tx, &session_id, -1).await;
            return;
        }
    };

    let container_id = format!("orca-{service_name}");
    let exec = match docker
        .create_exec(
            &container_id,
            CreateExecOptions {
                cmd: Some(cmd),
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(true),
                ..Default::default()
            },
        )
        .await
    {
        Ok(e) => e,
        Err(e) => {
            error!("exec: create_exec failed for {container_id}: {e}");
            send_done(&out_tx, &session_id, -1).await;
            return;
        }
    };

    if let Err(e) = docker
        .resize_exec(
            &exec.id,
            ResizeExecOptions {
                height: rows,
                width: cols,
            },
        )
        .await
    {
        warn!("exec: resize failed (non-fatal): {e}");
    }

    let result = docker.start_exec(&exec.id, None).await;

    let (mut stdin_writer, mut stdout_reader) = match result {
        Ok(StartExecResults::Attached { input, output }) => (input, output),
        Ok(StartExecResults::Detached) => {
            error!("exec: expected attached mode");
            send_done(&out_tx, &session_id, -1).await;
            return;
        }
        Err(e) => {
            error!("exec: start_exec failed: {e}");
            send_done(&out_tx, &session_id, -1).await;
            return;
        }
    };

    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    sessions.write().await.insert(session_id.clone(), stdin_tx);

    info!("exec: session {session_id} started on {container_id}");

    let out_tx2 = out_tx.clone();
    let sid2 = session_id.clone();
    tokio::spawn(async move {
        while let Some(Ok(msg)) = stdout_reader.next().await {
            let bytes = match msg {
                bollard::container::LogOutput::Console { message } => message,
                bollard::container::LogOutput::StdOut { message } => message,
                bollard::container::LogOutput::StdErr { message } => message,
                _ => continue,
            };
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            if out_tx2
                .send(AgentMessage::ExecOutput {
                    session_id: sid2.clone(),
                    data,
                })
                .await
                .is_err()
            {
                break;
            }
        }
        send_done(&out_tx2, &sid2, 0).await;
    });

    while let Some(data) = stdin_rx.recv().await {
        if stdin_writer.write_all(&data).await.is_err() {
            break;
        }
    }

    sessions.write().await.remove(&session_id);
}

/// Forward decoded stdin bytes to a running exec session.
pub async fn send_input(session_id: &str, data: &str, sessions: &ExecSessions) {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(data) {
        Ok(b) => b,
        Err(e) => {
            warn!("exec: bad base64 in ExecInput: {e}");
            return;
        }
    };
    let sessions = sessions.read().await;
    if let Some(tx) = sessions.get(session_id) {
        let _ = tx.send(bytes).await;
    }
}

/// Resize the PTY for an active exec session.
pub async fn resize(session_id: &str, cols: u16, rows: u16) {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            warn!("exec: docker connect for resize failed: {e}");
            return;
        }
    };
    if let Err(e) = docker
        .resize_exec(
            session_id,
            ResizeExecOptions {
                height: rows,
                width: cols,
            },
        )
        .await
    {
        warn!("exec: resize failed: {e}");
    }
}

/// Drop the stdin sender to signal the exec task to exit.
pub async fn close(session_id: &str, sessions: &ExecSessions) {
    sessions.write().await.remove(session_id);
}

async fn send_done(tx: &mpsc::Sender<AgentMessage>, session_id: &str, exit_code: i64) {
    let _ = tx
        .send(AgentMessage::ExecDone {
            session_id: session_id.to_string(),
            exit_code,
        })
        .await;
}
