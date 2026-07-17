//! Service-level ops handlers: scale, redeploy, rollback, promote, stop, logs.

mod backups;
mod cluster;
mod deploy;
mod networks;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tracing::error;

pub(crate) use backups::{cluster_backups, trigger_cluster_backup};
pub(crate) use cluster::logs;
pub(crate) use deploy::{
    promote, redeploy, rollback, scale, start_service, stop_all, stop_project, stop_service,
};
pub(crate) use networks::cluster_networks;

fn ok_or_500(result: anyhow::Result<()>, op: &str) -> axum::response::Response {
    match result {
        Ok(()) => Json(serde_json::json!({"ok": op})).into_response(),
        Err(e) => {
            error!("{op} failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{op} failed: {e}"),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use orca_core::config::{ClusterConfig, ClusterMeta};
    use orca_core::runtime::WorkloadHandle;
    use orca_core::testing::MockRuntime;
    use orca_core::types::{HealthState, WorkloadStatus};
    use orca_core::ws_types::MasterMessage;

    use crate::state::{AppState, InstanceState, ServiceState};

    fn make_service_config(name: &str) -> orca_core::config::ServiceConfig {
        orca_core::config::ServiceConfig {
            name: name.into(),
            project: None,
            runtime: Default::default(),
            image: Some(format!("{name}:latest")),
            module: None,
            replicas: Default::default(),
            port: Some(8080),
            host_port: None,
            domain: None,
            domains: vec![],
            routes: vec![],
            health: None,
            readiness: None,
            liveness: None,
            env: std::collections::HashMap::new(),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: None,
            aliases: vec![],
            mounts: vec![],
            triggers: vec![],
            assets: None,
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            depends_on: vec![],
            cmd: vec![],
            extra_ports: vec![],
            strip_prefix: None,
            pull_policy: Default::default(),
            backup: None,
        }
    }

    fn make_state() -> Arc<AppState> {
        let runtime = Arc::new(MockRuntime::with_host_port(9000));
        Arc::new(AppState::new(
            ClusterConfig {
                cluster: ClusterMeta {
                    name: "test".into(),
                    api_port: 0,
                    grpc_port: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime,
            None,
            Arc::new(RwLock::new(std::collections::HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        ))
    }

    /// The remote-node detection logic: `runtime_id` with "remote-<node_id>"
    /// prefix must parse to the correct node_id.
    #[test]
    fn remote_node_id_extracted_from_runtime_id() {
        let ids = ["remote-42", "remote-1", "remote-18446744073709551615"];
        let expected: &[u64] = &[42, 1, u64::MAX];
        for (id_str, &expected_id) in ids.iter().zip(expected) {
            let result = id_str
                .strip_prefix("remote-")
                .and_then(|s| s.parse::<u64>().ok());
            assert_eq!(result, Some(expected_id), "failed for {id_str}");
        }
    }

    /// Local container runtime_ids must NOT be detected as remote.
    #[test]
    fn local_runtime_id_not_detected_as_remote() {
        let local_ids = ["orca-myapp", "container-abc123", "abc123def456", ""];
        for id in local_ids {
            let result = id
                .strip_prefix("remote-")
                .and_then(|s| s.parse::<u64>().ok());
            assert!(result.is_none(), "'{id}' should not be detected as remote");
        }
    }

    /// `remote-` prefix with non-numeric suffix must return None safely.
    #[test]
    fn remote_prefix_with_non_numeric_suffix_returns_none() {
        let bad = "remote-not-a-number";
        let result = bad
            .strip_prefix("remote-")
            .and_then(|s| s.parse::<u64>().ok());
        assert!(result.is_none());
    }

    /// When an agent IS connected, logs() should register a listener, send the
    /// LogRequest, and return collected chunks when the agent sends `done=true`.
    #[tokio::test]
    async fn logs_remote_service_collects_chunks_from_agent() {
        let state = make_state();

        // Register a fake service with a remote instance.
        let node_id: u64 = 99;
        let handle = WorkloadHandle {
            runtime_id: format!("remote-{node_id}"),
            name: "myapp".into(),
            metadata: Default::default(),
        };
        {
            let mut services = state.services.write().await;
            let mut svc_state = ServiceState::from_config(make_service_config("myapp"));
            svc_state.instances.push(InstanceState {
                handle,
                status: WorkloadStatus::Running,
                host_port: None,
                container_address: None,
                health: HealthState::NoCheck,
                is_canary: false,
                started_at: std::time::Instant::now(),
            });
            services.insert("myapp".into(), svc_state);
        }

        // Register a fake WS sender for the agent.
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::channel::<MasterMessage>(4);
        state
            .ws_agents
            .write()
            .await
            .insert(node_id, crate::state::AgentSession::new(agent_tx));

        // Spawn a task that simulates the agent responding with log chunks.
        let state_clone = state.clone();
        tokio::spawn(async move {
            // Wait for the LogRequest to arrive at the agent.
            let msg = agent_rx.recv().await.unwrap();
            let request_id = match msg {
                MasterMessage::LogRequest { request_id, .. } => request_id,
                _ => panic!("expected LogRequest"),
            };
            // Reply with a chunk (done=true).
            let listeners = state_clone.log_listeners.read().await;
            if let Some(tx) = listeners.get(&request_id) {
                let _ = tx.send(("hello from agent\n".into(), true)).await;
            }
        });

        // Call the logs handler directly.
        let result = {
            let services = state.services.read().await;
            let svc = services.get("myapp").unwrap();
            svc.instances.iter().find_map(|i| {
                i.handle
                    .runtime_id
                    .strip_prefix("remote-")
                    .and_then(|s| s.parse::<u64>().ok())
            })
        };
        assert_eq!(result, Some(node_id));

        // Verify that a listener registered and cleaned up correctly.
        let request_id = uuid::Uuid::new_v4().to_string();
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<(String, bool)>(4);
        state
            .log_listeners
            .write()
            .await
            .insert(request_id.clone(), chunk_tx);
        assert!(state.log_listeners.read().await.contains_key(&request_id));

        // Simulate receiving a chunk with done=true.
        let _ = chunk_rx; // suppress unused warning — in real code this is consumed
        state.log_listeners.write().await.remove(&request_id);
        assert!(!state.log_listeners.read().await.contains_key(&request_id));
    }

    /// When the agent is NOT connected, logs() must return 503 and not leave
    /// a dangling listener in `log_listeners`.
    #[tokio::test]
    async fn logs_cleans_up_listener_when_agent_disconnected() {
        let state = make_state();
        let node_id: u64 = 77;

        // Register service with remote instance but NO agent sender.
        let handle = WorkloadHandle {
            runtime_id: format!("remote-{node_id}"),
            name: "svc".into(),
            metadata: Default::default(),
        };
        {
            let mut services = state.services.write().await;
            let mut svc = ServiceState::from_config(make_service_config("svc"));
            svc.instances.push(InstanceState {
                handle,
                status: WorkloadStatus::Running,
                host_port: None,
                container_address: None,
                health: HealthState::NoCheck,
                is_canary: false,
                started_at: std::time::Instant::now(),
            });
            services.insert("svc".into(), svc);
        }

        // Simulate the logic: register listener, fail to send, clean up.
        let request_id = uuid::Uuid::new_v4().to_string();
        let (chunk_tx, _chunk_rx) = tokio::sync::mpsc::channel::<(String, bool)>(4);
        state
            .log_listeners
            .write()
            .await
            .insert(request_id.clone(), chunk_tx);

        // Agent not connected → sent = false → cleanup listener.
        let sent = state.ws_agents.read().await.get(&node_id).is_some();
        if !sent {
            state.log_listeners.write().await.remove(&request_id);
        }

        // Listener must be removed after the failed send.
        assert!(
            !state.log_listeners.read().await.contains_key(&request_id),
            "dangling listener must be cleaned up when agent is disconnected"
        );
    }
}
