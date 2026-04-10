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

use crate::grpc::AgentClient;

/// Run the WS connection loop with automatic reconnection.
///
/// On success, messages flow bidirectionally. On failure, falls back
/// to the HTTP heartbeat loop until the next reconnect attempt.
pub async fn run_ws_loop(
    leader_url: &str,
    node_id: u64,
    token: &str,
    runtime: Arc<dyn Runtime>,
    agent: Arc<AgentClient>,
    domain_tx: mpsc::Sender<(String, String, u16)>,
) {
    let ws_url = build_ws_url(leader_url, node_id, token);
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

    // Spawn heartbeat sender (every 5s)
    let rt = runtime.clone();
    let agent_c = agent.clone();
    let stats_c = stats_collector.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let msg = build_heartbeat(node_id, &rt, &agent_c, &stats_c).await;
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(_) => continue,
            };
            if ws_tx
                .send(tungstenite::Message::Text(json.into()))
                .await
                .is_err()
            {
                break; // connection lost
            }
        }
    });

    // Process incoming master messages
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
                if let Err(e) = handle_master_message(&text, runtime, agent, domain_tx).await {
                    warn!("Error handling master message: {e}");
                }
            }
            tungstenite::Message::Close(_) => break,
            _ => {} // ping/pong handled by tungstenite
        }
    }

    heartbeat_handle.abort();
    Ok(())
}

/// Process a single message from the master.
async fn handle_master_message(
    text: &str,
    runtime: &Arc<dyn Runtime>,
    agent: &Arc<AgentClient>,
    domain_tx: &mpsc::Sender<(String, String, u16)>,
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

            // Send result back (will be picked up by heartbeat or a
            // dedicated send — for now, we log it; the heartbeat reports
            // running status which covers most cases)
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
            request_id: _,
            service_name: _,
            tail: _,
            follow: _,
        } => {
            // TODO: implement log streaming in next task
            warn!("WS: log streaming not yet implemented");
        }
        MasterMessage::Ack { node_id } => {
            info!("WS: master acknowledged node {node_id}");
        }
        MasterMessage::Reconcile { expected } => {
            info!("WS: reconciling {} expected services", expected.len());
            reconcile_services(expected, runtime, agent, domain_tx).await;
        }
    }

    Ok(())
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
fn build_ws_url(leader_url: &str, node_id: u64, token: &str) -> String {
    let base = leader_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    format!("{base}/api/v1/ws/agent?token={token}&node_id={node_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ws_url_http() {
        let url = build_ws_url("http://46.225.100.82:6880", 123, "abc");
        assert_eq!(
            url,
            "ws://46.225.100.82:6880/api/v1/ws/agent?token=abc&node_id=123"
        );
    }

    #[test]
    fn build_ws_url_https() {
        let url = build_ws_url("https://orca.example.com", 42, "tok");
        assert_eq!(
            url,
            "wss://orca.example.com/api/v1/ws/agent?token=tok&node_id=42"
        );
    }
}
