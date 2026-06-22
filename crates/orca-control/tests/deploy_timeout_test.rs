//! Tests for the two-phase remote-deploy timeout (#88 / #94).
//!
//! The master waits for a short *receipt* ACK (agent got the command) and then
//! a long *completion* ACK (deploy finished). This ensures:
//! - an unreachable agent fails fast with a distinct message, and
//! - a real agent-side error (e.g. image-not-found) surfaces verbatim instead
//!   of being masked by a bare 30s timeout.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::state::{AppState, RegisteredNode};
use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::MockRuntime;
use orca_core::types::{PlacementConstraint, Replicas, RuntimeKind};
use orca_core::ws_types::MasterMessage;

fn make_state(cfg: ClusterConfig) -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::new());
    Arc::new(AppState::new(
        cfg,
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

fn config_placed_on(name: &str, node: &str) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        project: None,
        runtime: RuntimeKind::Container,
        image: Some("nginx:latest".into()),
        module: None,
        replicas: Replicas::Fixed(1),
        port: Some(8080),
        host_port: None,
        domain: None,
        routes: vec![],
        health: None,
        readiness: None,
        liveness: None,
        env: HashMap::new(),
        resources: None,
        volume: None,
        deploy: None,
        placement: Some(PlacementConstraint {
            labels: None,
            node: Some(node.into()),
            requires_gpu: None,
        }),
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

async fn register_node(state: &AppState, node_id: u64) {
    state.registered_nodes.write().await.insert(
        node_id,
        RegisteredNode {
            node_id,
            address: format!("node-{node_id}:6881"),
            labels: HashMap::new(),
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
}

/// When the agent is connected but never acknowledges receipt, the deploy must
/// fail with a distinct "did not acknowledge / unreachable" message after the
/// short ACK window — NOT the old opaque "timed out after 30 s".
#[tokio::test]
async fn ack_timeout_reports_unreachable_distinctly() {
    let mut cfg = ClusterConfig::default();
    cfg.deploy.ack_timeout_secs = 1; // keep the test fast
    let state = make_state(cfg);
    register_node(&state, 1).await;

    // Register a WS sender whose receiver we hold open, so the Deploy send
    // succeeds but nothing ever replies with DeployReceived.
    let (tx, _rx) = tokio::sync::mpsc::channel::<MasterMessage>(8);
    state.ws_agents.write().await.insert(1, tx);

    let cfg = config_placed_on("svc", "1");
    let (deployed, errors) = orca_control::reconciler::reconcile(&state, &[cfg]).await;

    assert!(deployed.is_empty(), "deploy should not be recorded");
    assert_eq!(errors.len(), 1, "expected one error, got {errors:?}");
    let err = &errors[0];
    assert!(
        err.contains("did not acknowledge") && err.contains("unreachable"),
        "expected a distinct unreachable message, got: {err}"
    );
    assert!(
        !err.contains("timed out after 30"),
        "must not emit the old opaque 30s timeout: {err}"
    );

    // Both waiter maps must be cleaned up so they don't leak.
    assert!(state.pending_deploy_acks.read().await.is_empty());
    assert!(state.pending_deploys.read().await.is_empty());
}

/// Once the agent acknowledges receipt, a real deploy failure (e.g. image not
/// found) must surface verbatim instead of being masked by a timeout.
#[tokio::test]
async fn real_agent_error_propagates_after_ack() {
    let state = make_state(ClusterConfig::default()); // default 10s/600s timeouts
    register_node(&state, 1).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<MasterMessage>(8);
    state.ws_agents.write().await.insert(1, tx);

    let cfg = config_placed_on("svc", "1");
    let state_c = state.clone();
    let deploy =
        tokio::spawn(async move { orca_control::reconciler::reconcile(&state_c, &[cfg]).await });

    // The master pushes the Deploy command; receiving it confirms both waiter
    // entries are registered.
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("master should send a Deploy")
        .expect("channel open");
    assert!(matches!(msg, MasterMessage::Deploy { .. }));

    // Simulate the agent: acknowledge receipt, then report a pull failure.
    state
        .pending_deploy_acks
        .write()
        .await
        .remove("svc")
        .expect("ack waiter registered")
        .send(())
        .ok();
    state
        .pending_deploys
        .write()
        .await
        .remove("svc")
        .expect("result waiter registered")
        .send(Err(
            "pull access denied for ghcr.io/x:nope, not found".into()
        ))
        .ok();

    let (deployed, errors) = deploy.await.unwrap();
    assert!(deployed.is_empty());
    assert_eq!(errors.len(), 1, "expected one error, got {errors:?}");
    let err = &errors[0];
    assert!(
        err.contains("not found") && err.contains("ghcr.io/x:nope"),
        "real pull error must propagate verbatim, got: {err}"
    );
    assert!(
        !err.contains("timed out") && !err.contains("did not"),
        "should not be a timeout once the agent reported a result: {err}"
    );
}
