//! #309: `orca stop` on an agent-placed service stops its containers there.

use std::collections::HashMap;
use std::sync::Arc;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::MockRuntime;
use orca_core::ws_types::MasterMessage;
use tokio::sync::{Notify, RwLock, mpsc};

use crate::operations::{start, stop};
use crate::session::AgentSession;
use crate::state::{AppState, RegisteredNode, ServiceState};

const AGENT: u64 = 42;

fn pinned(name: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:latest",
        "port": 8080,
        "placement": { "node": "agent-a" },
    }))
    .unwrap()
}

/// A master with one registered agent node running `web`.
async fn master() -> Arc<AppState> {
    let state = Arc::new(AppState::new(
        ClusterConfig::default(),
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));
    state.registered_nodes.write().await.insert(
        AGENT,
        RegisteredNode {
            node_id: AGENT,
            address: "agent-a:6881".into(),
            labels: HashMap::new(),
            peer_ip: None,
            last_heartbeat: chrono::Utc::now(),
            drain: false,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_total: 0,
            disk_used: 0,
            disk_total: 0,
            net_rx: 0,
            net_tx: 0,
        },
    );
    let mut web = ServiceState::from_config(pinned("web"));
    web.desired_replicas = 1;
    web.instances.push(crate::state::InstanceState {
        handle: orca_core::runtime::WorkloadHandle {
            runtime_id: format!("remote-{AGENT}"),
            name: "orca-web".into(),
            metadata: Default::default(),
        },
        status: orca_core::types::WorkloadStatus::Running,
        host_port: None,
        container_address: None,
        health: orca_core::types::HealthState::NoCheck,
        started_at: std::time::Instant::now(),
        is_canary: false,
    });
    state.services.write().await.insert("web".into(), web);
    state
}

/// Connect an agent that answers every stop with `stop_ok` and every deploy
/// with success, and records what it was sent.
async fn connect_agent(state: &Arc<AppState>, stop_ok: bool) -> Arc<RwLock<Vec<String>>> {
    let (tx, mut rx) = mpsc::channel(8);
    state.ws_agents.write().await.insert(
        AGENT,
        AgentSession {
            tx,
            session_id: 1,
            shutdown: Arc::new(Notify::new()),
        },
    );
    let seen = Arc::new(RwLock::new(Vec::new()));
    let (st, log) = (state.clone(), seen.clone());
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                MasterMessage::Stop { service_name } => {
                    log.write().await.push(format!("stop {service_name}"));
                    let result = if stop_ok { Ok(()) } else { Err("busy".into()) };
                    if let Some(done) = st.pending_stops.write().await.remove(&service_name) {
                        let _ = done.send(result);
                    }
                }
                MasterMessage::Deploy { spec } => {
                    log.write().await.push(format!("deploy {}", spec.name));
                    if let Some(ack) = st.pending_deploy_acks.write().await.remove(&spec.name) {
                        let _ = ack.send(());
                    }
                    if let Some(done) = st.pending_deploys.write().await.remove(&spec.name) {
                        let _ = done.send(Ok(()));
                    }
                }
                _ => {}
            }
        }
    });
    seen
}

#[tokio::test]
async fn stop_reaches_the_agent_and_start_deploys_again() {
    let state = master().await;
    let seen = connect_agent(&state, true).await;

    stop(&state, "web").await.unwrap();
    {
        let services = state.services.read().await;
        assert!(services["web"].stopped);
        assert!(
            services["web"].instances.is_empty(),
            "no running placeholder"
        );
    }

    start(&state, "web").await.unwrap();

    assert_eq!(*seen.read().await, ["stop web", "deploy web"]);
}

#[tokio::test]
async fn a_stop_the_agent_could_not_do_is_reported_and_the_service_stays_running() {
    let state = master().await;
    connect_agent(&state, false).await;

    let err = stop(&state, "web").await.unwrap_err().to_string();

    assert!(err.contains("busy"), "{err}");
    let services = state.services.read().await;
    assert!(!services["web"].stopped);
    assert_eq!(services["web"].desired_replicas, 1);
    assert_eq!(services["web"].instances.len(), 1, "it still runs there");
}

#[tokio::test]
async fn an_offline_agent_gets_the_stop_when_it_reconnects() {
    let state = master().await;

    stop(&state, "web").await.unwrap();

    let pending = state.pending_commands.read().await;
    assert_eq!(pending[&AGENT][0]["action"], "stop");
    assert_eq!(pending[&AGENT][0]["service_name"], "web");
}

#[tokio::test]
async fn a_service_on_a_drained_node_can_still_be_stopped() {
    // Drained before a migration; deploy targeting refuses drained nodes.
    let state = master().await;
    state
        .services
        .write()
        .await
        .get_mut("web")
        .unwrap()
        .instances
        .clear();
    state
        .registered_nodes
        .write()
        .await
        .get_mut(&AGENT)
        .unwrap()
        .drain = true;
    let seen = connect_agent(&state, true).await;

    stop(&state, "web").await.unwrap();

    assert_eq!(*seen.read().await, ["stop web"]);
}

#[tokio::test]
async fn a_node_that_left_the_registry_gets_the_stop_queued_by_its_placeholder() {
    let state = master().await;
    state.registered_nodes.write().await.clear();

    stop(&state, "web").await.unwrap();

    assert_eq!(
        state.pending_commands.read().await[&AGENT][0]["action"],
        "stop"
    );
}

#[tokio::test(start_paused = true)]
async fn an_unconfirmed_stop_is_an_error_not_a_pause() {
    // An agent older than v0.3.2: it gets the stop and never answers.
    let state = master().await;
    let (tx, _rx) = mpsc::channel(8);
    state.ws_agents.write().await.insert(
        AGENT,
        AgentSession {
            tx,
            session_id: 1,
            shutdown: Arc::new(Notify::new()),
        },
    );

    let err = stop(&state, "web").await.unwrap_err().to_string();

    assert!(err.contains("did not confirm"), "{err}");
    assert!(!state.services.read().await["web"].stopped);
}

#[tokio::test]
async fn rm_refuses_while_the_agent_is_offline_unless_forced() {
    // The record would be gone while the container runs on; the adoption
    // scan would find it again.
    let state = master().await;

    let err = crate::removal::remove(&state, "web", false)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("offline"), "{err:#}");
    assert!(state.services.read().await.contains_key("web"));

    crate::removal::remove(&state, "web", true).await.unwrap();
    assert!(state.services.read().await.is_empty());
}

#[tokio::test]
async fn rm_on_a_connected_agent_stops_it_there() {
    let state = master().await;
    let seen = connect_agent(&state, true).await;
    state.deploy_history.write().await.record(&pinned("web"));
    state.deploy_history.write().await.record(&pinned("web"));

    crate::removal::remove(&state, "web", false).await.unwrap();

    assert_eq!(*seen.read().await, ["stop web"]);
    let err = crate::operations::rollback(&state, "web")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no previous deploy"),
        "rm forgets the history"
    );
}
