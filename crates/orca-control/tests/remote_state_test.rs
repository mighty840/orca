//! Regression tests for remote placeholder instance state management.
//!
//! Verifies that watchdog, health checker, and WS DeployResult handler
//! all correctly handle remote-{node_id} placeholder instances.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite;

use orca_control::health::HealthChecker;
use orca_control::state::{AppState, InstanceState, ServiceState};
use orca_control::watchdog::run_watchdog_cycle;
use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::runtime::WorkloadHandle;
use orca_core::testing::MockRuntime;
use orca_core::types::{HealthState, PlacementConstraint, Replicas, RuntimeKind, WorkloadStatus};
use orca_core::ws_types::AgentMessage;

fn make_state(token: &str) -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::new());
    let mut cfg = ClusterConfig::default();
    cfg.api_tokens = vec![token.into()];
    Arc::new(AppState::new(
        cfg,
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

fn make_config(name: &str) -> ServiceConfig {
    ServiceConfig {
        restart_policy: None,
        name: name.into(),
        project: None,
        runtime: RuntimeKind::Container,
        image: Some("nginx:latest".into()),
        module: None,
        replicas: Replicas::Fixed(1),
        port: Some(8080),
        host_port: None,
        domain: None,
        domains: vec![],
        routes: vec![],
        health: None,
        readiness: None,
        liveness: None,
        env: HashMap::new(),
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

fn remote_instance(node_id: u64, status: WorkloadStatus) -> InstanceState {
    InstanceState {
        handle: WorkloadHandle {
            runtime_id: format!("remote-{node_id}"),
            name: format!("remote-{node_id}"),
            metadata: HashMap::new(),
        },
        status,
        host_port: None,
        container_address: None,
        health: HealthState::Healthy,
        is_canary: false,
        started_at: std::time::Instant::now() - Duration::from_secs(10),
    }
}

/// Watchdog must not prune remote placeholder instances even when their
/// cached status is Stopped. The heartbeat handler owns their lifecycle.
#[tokio::test]
async fn watchdog_does_not_prune_remote_placeholder() {
    let state = make_state("tok");
    {
        let mut services = state.services.write().await;
        let mut svc = ServiceState::from_config(make_config("rem-svc"));
        svc.instances
            .push(remote_instance(42, WorkloadStatus::Stopped));
        services.insert("rem-svc".into(), svc);
    }

    run_watchdog_cycle(&state).await;

    let services = state.services.read().await;
    let svc = services.get("rem-svc").expect("service must still exist");
    assert_eq!(
        svc.instances.len(),
        1,
        "remote placeholder must not be pruned"
    );
    assert_eq!(svc.instances[0].handle.runtime_id, "remote-42");
}

/// When a service has only a remote placeholder (even with status Stopped),
/// the watchdog must not trigger local reconciliation and create new instances.
#[tokio::test]
async fn watchdog_no_spurious_reconcile_for_remote_service() {
    let state = make_state("tok");
    {
        let mut services = state.services.write().await;
        let mut svc = ServiceState::from_config(make_config("rem-svc2"));
        svc.instances
            .push(remote_instance(99, WorkloadStatus::Stopped));
        services.insert("rem-svc2".into(), svc);
    }

    run_watchdog_cycle(&state).await;

    let services = state.services.read().await;
    let svc = services.get("rem-svc2").unwrap();
    assert_eq!(
        svc.instances.len(),
        1,
        "reconcile must not create additional local instances"
    );
    assert!(
        svc.instances[0].handle.runtime_id.starts_with("remote-"),
        "only the original remote placeholder should remain"
    );
}

/// DeployResult success from an agent must flip the remote placeholder's
/// status from Stopped to Running.
#[tokio::test]
async fn deploy_result_updates_remote_instance_to_running() {
    let state = make_state("test-tok");
    {
        let mut services = state.services.write().await;
        let mut svc = ServiceState::from_config(make_config("my-svc"));
        svc.instances
            .push(remote_instance(7, WorkloadStatus::Stopped));
        services.insert("my-svc".into(), svc);
    }

    let app = axum::Router::new()
        .route(
            "/api/v1/ws/agent",
            axum::routing::get(orca_control::ws_handler::ws_agent_handler),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let url = format!("ws://{addr}/api/v1/ws/agent?token=test-tok&node_id=7");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let _ = ws.next().await; // consume Ack

    let msg = AgentMessage::DeployResult {
        service_name: "my-svc".into(),
        success: true,
        error: None,
    };
    ws.send(tungstenite::Message::Text(
        serde_json::to_string(&msg).unwrap().into(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let services = state.services.read().await;
    assert_eq!(
        services["my-svc"].instances[0].status,
        WorkloadStatus::Running,
        "DeployResult success must set remote placeholder to Running"
    );
    ws.close(None).await.ok();
}

/// Health checker must not record failures for remote placeholder instances.
/// Without the guard, it calls runtime.status on a remote handle (which the
/// local Docker daemon doesn't know about) → returns Failed → increments
/// failure_counts → eventually triggers a spurious restart.
#[tokio::test]
async fn health_checker_skips_remote_instance() {
    let state = make_state("tok");
    {
        let mut services = state.services.write().await;
        let mut svc = ServiceState::from_config(make_config("hc-svc"));
        svc.instances
            .push(remote_instance(55, WorkloadStatus::Running));
        services.insert("hc-svc".into(), svc);
    }

    let checker = HealthChecker::new(state);
    let mut failure_counts = HashMap::new();
    checker.check_all(&mut failure_counts).await;

    assert!(
        !failure_counts.contains_key("remote-55"),
        "health checker must not record failures for remote instances"
    );
}

/// A service with placement.node set and zero instances (master startup state)
/// must not trigger local reconciliation — the placement guard in check_and_prune
/// must return false before the current < desired comparison.
#[tokio::test]
async fn watchdog_placement_guard_prevents_reconcile_with_zero_instances() {
    let state = make_state("tok");
    {
        let mut services = state.services.write().await;
        let mut config = make_config("placed-svc");
        config.placement = Some(PlacementConstraint {
            node: Some("node-99".into()),
            labels: None,
            requires_gpu: None,
        });
        let svc = ServiceState::from_config(config);
        // Intentionally NO instances — mirrors restore_or_reconcile startup state.
        services.insert("placed-svc".into(), svc);
    }

    run_watchdog_cycle(&state).await;

    let services = state.services.read().await;
    let svc = services.get("placed-svc").unwrap();
    assert_eq!(
        svc.instances.len(),
        0,
        "placement guard must prevent local reconcile even with zero instances"
    );
}
