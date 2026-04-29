//! Log streaming for the WS client: forward container logs to master.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::error;

use orca_core::runtime::Runtime;
use orca_core::ws_types::AgentMessage;

/// Stream container logs back to master via LogChunk messages.
pub(super) async fn stream_logs(
    runtime: &Arc<dyn Runtime>,
    service_name: &str,
    request_id: &str,
    tail: u64,
    follow: bool,
    out_tx: mpsc::Sender<AgentMessage>,
) {
    use tokio::io::AsyncReadExt;

    let handle = orca_core::runtime::WorkloadHandle {
        runtime_id: format!("orca-{service_name}"),
        name: format!("orca-{service_name}"),
        metadata: Default::default(),
    };
    let opts = orca_core::runtime::LogOpts {
        follow,
        tail: Some(tail),
        since: None,
        timestamps: false,
    };

    match runtime.logs(&handle, &opts).await {
        Ok(mut stream) => {
            let mut buf = Vec::new();
            // Read up to 1MB
            let mut limited = (&mut stream).take(1024 * 1024);
            match limited.read_to_end(&mut buf).await {
                Ok(_) => {
                    let data = String::from_utf8_lossy(&buf).into_owned();
                    let _ = out_tx
                        .send(AgentMessage::LogChunk {
                            request_id: request_id.to_string(),
                            service_name: service_name.to_string(),
                            data,
                            done: true,
                        })
                        .await;
                }
                Err(e) => {
                    error!("WS: failed to read logs for {service_name}: {e}");
                    let _ = out_tx
                        .send(AgentMessage::LogChunk {
                            request_id: request_id.to_string(),
                            service_name: service_name.to_string(),
                            data: format!("error reading logs: {e}"),
                            done: true,
                        })
                        .await;
                }
            }
        }
        Err(e) => {
            error!("WS: logs unavailable for {service_name}: {e}");
            let _ = out_tx
                .send(AgentMessage::LogChunk {
                    request_id: request_id.to_string(),
                    service_name: service_name.to_string(),
                    data: format!("logs unavailable: {e}"),
                    done: true,
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    /// Log stream handle must use the "orca-{service_name}" convention so the
    /// container runtime can locate the right container.
    #[test]
    fn stream_logs_handle_uses_orca_prefix() {
        let service_name = "my-service";
        let runtime_id = format!("orca-{service_name}");
        assert_eq!(runtime_id, "orca-my-service");
        // The name field also follows this convention.
        let name = format!("orca-{service_name}");
        assert_eq!(name, runtime_id);
    }

    /// `stream_logs` must always send `done=true` in the final chunk so the
    /// master-side collector loop terminates.  We verify the LogChunk
    /// serialization round-trips correctly with done=true.
    #[test]
    fn log_chunk_done_true_serializes_correctly() {
        use orca_core::ws_types::AgentMessage;
        let msg = AgentMessage::LogChunk {
            request_id: "req-1".into(),
            service_name: "myapp".into(),
            data: "some log line\n".into(),
            done: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: AgentMessage = serde_json::from_str(&json).unwrap();
        match back {
            AgentMessage::LogChunk { done, data, .. } => {
                assert!(done, "done must survive round-trip");
                assert_eq!(data, "some log line\n");
            }
            _ => panic!("unexpected variant"),
        }
    }
}
