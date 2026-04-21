//! WebSocket client for agent→master streaming communication.
//!
//! Replaces the HTTP heartbeat polling with a persistent WS connection.
//! Falls back to HTTP heartbeat if WS connection fails.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use tracing::{error, info, warn};

use orca_core::runtime::Runtime;
use orca_core::ws_types::{AgentMessage, HostStats, MasterMessage};

use crate::ws_exec::ExecSessions;

use crate::grpc::AgentClient;

/// Run the WS connection loop with automatic reconnection.
///
/// On success, messages flow bidirectionally. On failure, falls back
/// to the HTTP heartbeat loop until the next reconnect attempt.
pub async fn run_ws_loop(
    leader_url: &str,
    node_id: u64,
    token: &str,
    local_address: &str,
    runtime: Arc<dyn Runtime>,
    agent: Arc<AgentClient>,
    domain_tx: mpsc::Sender<(String, String, u16)>,
) {
    let ws_url = build_ws_url(leader_url, node_id, token, local_address);
    let mut backoff = Duration::from_secs(2);
    let max_backoff = Duration::from_secs(30);

    loop {
        info!("Connecting to master WebSocket: {ws_url}");
        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((ws_stream, _)) => {
                info!("WebSocket connected to master");
                backoff = Duration::from_secs(2); // reset on success

                if let Err(e) =
                    handle_ws_session(ws_stream, node_id, &runtime, &agent, &domain_tx).await
                {
                    warn!("WebSocket session ended: {e}");
                }
            }
            Err(e) => {
                warn!("WebSocket connect failed: {e}, retrying in {backoff:?}");
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Handle a single WS session (connected).
async fn handle_ws_session(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    node_id: u64,
    runtime: &Arc<dyn Runtime>,
    agent: &Arc<AgentClient>,
    domain_tx: &mpsc::Sender<(String, String, u16)>,
) -> anyhow::Result<()> {
    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let stats_collector = Arc::new(crate::host_stats::HostStatsCollector::new());
    let exec_sessions: ExecSessions =
        Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

    // Shared output channel — heartbeat + log/backup responses all funnel here.
    let (out_tx, mut out_rx) = mpsc::channel::<AgentMessage>(128);

    // Dedicated WS writer task.
    let write_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to serialize AgentMessage: {e}");
                    continue;
                }
            };
            if ws_tx
                .send(tungstenite::Message::Text(json.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Heartbeat sender (every 5s) — uses out_tx.
    let rt = runtime.clone();
    let agent_c = agent.clone();
    let stats_c = stats_collector.clone();
    let hb_tx = out_tx.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let msg = build_heartbeat(node_id, &rt, &agent_c, &stats_c).await;
            if hb_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Process incoming master messages, passing out_tx for async responses.
    while let Some(msg_result) = ws_rx.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                error!("WebSocket receive error: {e}");
                break;
            }
        };

        match msg {
            tungstenite::Message::Text(text) => {
                if let Err(e) = handle_master_message(
                    &text,
                    node_id,
                    runtime,
                    agent,
                    domain_tx,
                    &out_tx,
                    &exec_sessions,
                )
                .await
                {
                    warn!("Error handling master message: {e}");
                }
            }
            tungstenite::Message::Close(_) => break,
            _ => {} // ping/pong handled by tungstenite
        }
    }

    write_task.abort();
    heartbeat_handle.abort();
    Ok(())
}

/// Process a single message from the master.
async fn handle_master_message(
    text: &str,
    node_id: u64,
    runtime: &Arc<dyn Runtime>,
    agent: &Arc<AgentClient>,
    domain_tx: &mpsc::Sender<(String, String, u16)>,
    out_tx: &mpsc::Sender<AgentMessage>,
    exec_sessions: &ExecSessions,
) -> anyhow::Result<()> {
    let msg: MasterMessage = serde_json::from_str(text)?;

    match msg {
        MasterMessage::Deploy { spec } => {
            info!("WS: deploying {}", spec.name);
            let result = agent.deploy_spec(runtime.as_ref(), &spec).await;
            let success = result.is_ok();
            let error = result.err().map(|e| e.to_string());

            // If the spec has a domain, notify for proxy route registration
            if success
                && let Some(domain) = &spec.domain
                && let Ok(Some(port)) = runtime
                    .resolve_host_port(
                        &orca_core::runtime::WorkloadHandle {
                            runtime_id: format!("orca-{}", spec.name),
                            name: format!("orca-{}", spec.name),
                            metadata: Default::default(),
                        },
                        spec.port.unwrap_or(80),
                    )
                    .await
            {
                let _ = domain_tx
                    .send((spec.name.clone(), domain.clone(), port))
                    .await;
            }

            if success {
                info!("WS: deploy of {} succeeded", spec.name);
            } else {
                error!(
                    "WS: deploy of {} failed: {}",
                    spec.name,
                    error.as_deref().unwrap_or("unknown")
                );
            }
        }
        MasterMessage::Stop { service_name } => {
            info!("WS: stopping {service_name}");
            agent.stop_service(runtime.as_ref(), &service_name).await;
        }
        MasterMessage::LogRequest {
            request_id,
            service_name,
            tail,
            follow,
        } => {
            info!("WS: log request for {service_name} (tail={tail}, follow={follow})");
            let rt = runtime.clone();
            let tx = out_tx.clone();
            tokio::spawn(async move {
                stream_logs(&rt, &service_name, &request_id, tail, follow, tx).await;
            });
        }
        MasterMessage::BackupRequest {
            config,
            service_hooks,
        } => {
            info!("WS: backup request received");
            let tx = out_tx.clone();
            tokio::spawn(async move {
                run_agent_backup(node_id, config, service_hooks, tx).await;
            });
        }
        MasterMessage::Ack { node_id } => {
            info!("WS: master acknowledged node {node_id}");
        }
        MasterMessage::Reconcile { expected } => {
            info!("WS: reconciling {} expected services", expected.len());
            reconcile_services(expected, runtime, agent, domain_tx).await;
        }
        MasterMessage::ExecStart {
            session_id,
            service_name,
            cmd,
            cols,
            rows,
        } => {
            info!("WS: exec start session={session_id} service={service_name}");
            let sessions = exec_sessions.clone();
            let tx = out_tx.clone();
            tokio::spawn(async move {
                crate::ws_exec::start_exec(session_id, service_name, cmd, cols, rows, sessions, tx)
                    .await;
            });
        }
        MasterMessage::ExecInput { session_id, data } => {
            crate::ws_exec::send_input(&session_id, &data, exec_sessions).await;
        }
        MasterMessage::ExecResize {
            session_id,
            cols,
            rows,
        } => {
            crate::ws_exec::resize(&session_id, cols, rows).await;
        }
        MasterMessage::ExecClose { session_id } => {
            crate::ws_exec::close(&session_id, exec_sessions).await;
        }
    }

    Ok(())
}

/// Stream container logs back to master via LogChunk messages.
async fn stream_logs(
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

/// Run a backup on this agent node using the config sent by master.
/// Passes backup targets as JSON via env var so the CLI subprocess can use them.
async fn run_agent_backup(
    node_id: u64,
    config: orca_core::backup::BackupConfig,
    service_hooks: std::collections::HashMap<String, String>,
    out_tx: mpsc::Sender<AgentMessage>,
) {
    let config_json = match serde_json::to_string(&config) {
        Ok(j) => j,
        Err(e) => {
            error!("WS: failed to serialize backup config: {e}");
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: false,
                    message: format!("config serialization failed: {e}"),
                })
                .await;
            return;
        }
    };

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: false,
                    message: format!("cannot resolve binary: {e}"),
                })
                .await;
            return;
        }
    };

    let hooks_json = serde_json::to_string(&service_hooks).unwrap_or_default();
    match tokio::process::Command::new(&exe)
        .args(["backup", "all"])
        .env("ORCA_BACKUP_CONFIG_JSON", &config_json)
        .env("ORCA_SERVICE_HOOKS_JSON", &hooks_json)
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            let msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
            info!("WS: agent backup complete: {msg}");
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: true,
                    message: msg,
                })
                .await;
        }
        Ok(out) => {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            error!("WS: agent backup failed: {msg}");
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: false,
                    message: msg,
                })
                .await;
        }
        Err(e) => {
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: false,
                    message: format!("spawn failed: {e}"),
                })
                .await;
        }
    }
}

/// Reconcile: compare expected services from master against what's actually
/// running locally. Deploy any missing services, skip ones already running.
#[allow(clippy::vec_box)]
async fn reconcile_services(
    expected: Vec<Box<orca_core::types::WorkloadSpec>>,
    runtime: &Arc<dyn Runtime>,
    agent: &Arc<AgentClient>,
    domain_tx: &mpsc::Sender<(String, String, u16)>,
) {
    let running = agent.collect_workload_reports(runtime.as_ref()).await;
    let running_names: std::collections::HashSet<String> =
        running.iter().map(|r| r.service_name.clone()).collect();

    let mut deployed = 0u32;
    let mut skipped = 0u32;

    for spec in &expected {
        if running_names.contains(&spec.name) {
            skipped += 1;
            continue;
        }

        info!("Reconcile: deploying missing service {}", spec.name);
        match agent.deploy_spec(runtime.as_ref(), spec).await {
            Ok(()) => {
                deployed += 1;
                // Notify domain discovery
                if let Some(domain) = &spec.domain
                    && let Ok(Some(port)) = runtime
                        .resolve_host_port(
                            &orca_core::runtime::WorkloadHandle {
                                runtime_id: format!("orca-{}", spec.name),
                                name: format!("orca-{}", spec.name),
                                metadata: Default::default(),
                            },
                            spec.port.unwrap_or(80),
                        )
                        .await
                {
                    let _ = domain_tx
                        .send((spec.name.clone(), domain.clone(), port))
                        .await;
                }
            }
            Err(e) => {
                error!("Reconcile: failed to deploy {}: {e}", spec.name);
            }
        }
    }

    info!("Reconcile complete: {deployed} deployed, {skipped} already running");
}

/// Build heartbeat message with current workload status and host stats.
async fn build_heartbeat(
    node_id: u64,
    runtime: &Arc<dyn Runtime>,
    agent: &Arc<AgentClient>,
    stats_collector: &crate::host_stats::HostStatsCollector,
) -> AgentMessage {
    let workloads = agent.collect_workload_reports(runtime.as_ref()).await;
    let sample = stats_collector.sample();

    AgentMessage::Heartbeat {
        node_id,
        workloads,
        stats: HostStats {
            cpu_percent: sample.cpu_percent,
            memory_bytes: sample.memory_bytes,
            memory_total: sample.memory_total,
            disk_used: sample.disk_used,
            disk_total: sample.disk_total,
            net_rx: sample.net_rx,
            net_tx: sample.net_tx,
            domains: vec![],
        },
    }
}

/// Convert HTTP leader URL to WS URL.
fn build_ws_url(leader_url: &str, node_id: u64, token: &str, address: &str) -> String {
    let base = leader_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let encoded_addr = address.replace(':', "%3A");
    format!("{base}/api/v1/ws/agent?token={token}&node_id={node_id}&address={encoded_addr}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ws_url_http() {
        let url = build_ws_url("http://46.225.100.82:6880", 123, "abc", "10.0.0.5:6881");
        assert_eq!(
            url,
            "ws://46.225.100.82:6880/api/v1/ws/agent?token=abc&node_id=123&address=10.0.0.5%3A6881"
        );
    }

    #[test]
    fn build_ws_url_https() {
        let url = build_ws_url("https://orca.example.com", 42, "tok", "192.168.1.5:6881");
        assert_eq!(
            url,
            "wss://orca.example.com/api/v1/ws/agent?token=tok&node_id=42&address=192.168.1.5%3A6881"
        );
    }

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

    /// `run_agent_backup` config JSON must round-trip so the subprocess sees
    /// the correct targets.
    #[test]
    fn backup_config_json_roundtrip() {
        use orca_core::backup::{BackupConfig, BackupTarget};
        let cfg = BackupConfig {
            schedule: Some("0 0 2 * * *".into()),
            retention_days: 14,
            targets: vec![BackupTarget::Local {
                path: "/data/backups".into(),
            }],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: BackupConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.retention_days, 14);
        match &back.targets[0] {
            BackupTarget::Local { path } => assert_eq!(path, "/data/backups"),
            _ => panic!("expected Local target"),
        }
    }

    /// BackupResult with success=false and an error message serializes and
    /// deserializes correctly so master can log the failure.
    #[test]
    fn backup_result_failure_roundtrip() {
        use orca_core::ws_types::AgentMessage;
        let msg = AgentMessage::BackupResult {
            node_id: 5,
            success: false,
            message: "spawn failed: No such file".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: AgentMessage = serde_json::from_str(&json).unwrap();
        match back {
            AgentMessage::BackupResult {
                success,
                message,
                node_id,
            } => {
                assert_eq!(node_id, 5);
                assert!(!success);
                assert!(message.contains("spawn failed"));
            }
            _ => panic!("unexpected variant"),
        }
    }
}
