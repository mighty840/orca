//! Tests for K8s-style failure reasons surfaced in `orca status`.
//!
//! Covers: a crash heartbeat records a per-service failure, and the status
//! endpoint surfaces `last_failure` only while the service is degraded.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite;

use orca_control::api::router;
use orca_control::state::{AppState, InstanceState, ServiceState};
use orca_core::api_types::FailureInfo;
use orca_core::config::{ClusterConfig, ClusterMeta, ServiceConfig};
use orca_core::runtime::WorkloadHandle;
use orca_core::testing::MockRuntime;
use orca_core::types::{HealthState, Replicas, RuntimeKind, WorkloadStatus};
use orca_core::ws_types::{AgentMessage, HostStats, WorkloadReport};
use tower::ServiceExt;

fn state_with_token(token: &str) -> Arc<AppState> {
    Arc::new(AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "fail-test".into(),
                api_port: 0,
                grpc_port: 0,
                ..Default::default()
            },
            api_tokens: vec![token.into()],
            ..Default::default()
        },
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

fn config(name: &str) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        project: None,
        runtime: RuntimeKind::Container,
        image: Some("nginx:latest".into()),
        module: None,
        replicas: Replicas::Fixed(1),
        port: Some(80),
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

fn running_instance() -> InstanceState {
    InstanceState {
        handle: WorkloadHandle {
            runtime_id: "remote-7".into(),
            name: "remote-7".into(),
            metadata: HashMap::new(),
        },
        status: WorkloadStatus::Running,
        host_port: None,
        container_address: None,
        health: HealthState::NoCheck,
        is_canary: false,
        started_at: std::time::Instant::now(),
    }
}

async fn status_json(state: &Arc<AppState>) -> serde_json::Value {
    let req = Request::get("/api/v1/status")
        .header("authorization", "Bearer tok")
        .body(Body::empty())
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn status_surfaces_failure_only_when_degraded() {
    let state = state_with_token("tok");
    {
        let mut services = state.services.write().await;
        // Degraded: desired 1, no running instances.
        services.insert("broken".into(), ServiceState::from_config(config("broken")));
        // Healthy: desired 1 with a running instance.
        let mut healthy = ServiceState::from_config(config("healthy"));
        healthy.instances.push(running_instance());
        services.insert("healthy".into(), healthy);
    }
    // Both have a recorded failure...
    {
        let mut f = state.last_failures.write().await;
        for name in ["broken", "healthy"] {
            f.insert(
                name.into(),
                FailureInfo {
                    reason: "ImagePullError".into(),
                    message: "pull access denied, not found".into(),
                    exit_code: None,
                    restart_count: 0,
                    observed_at: chrono::Utc::now(),
                },
            );
        }
    }

    let json = status_json(&state).await;
    let by_name: HashMap<&str, &serde_json::Value> = json["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s))
        .collect();

    // Degraded service exposes the failure.
    assert_eq!(
        by_name["broken"]["last_failure"]["reason"], "ImagePullError",
        "degraded service should surface its failure"
    );
    // Healthy service hides it even though an entry exists.
    assert!(
        by_name["healthy"]["last_failure"].is_null(),
        "healthy service must not surface a stale failure"
    );
}

#[tokio::test]
async fn crash_heartbeat_records_failure() {
    let state = state_with_token("tok");

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

    let url = format!("ws://{addr}/api/v1/ws/agent?token=tok&node_id=7");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let _ = ws.next().await; // Ack

    // Heartbeat reporting a crashed container with detail.
    let hb = AgentMessage::Heartbeat {
        node_id: 7,
        workloads: vec![WorkloadReport {
            service_name: "crashy".into(),
            status: "failed".into(),
            container_id: Some("c1".into()),
            cpu_percent: 0.0,
            memory_bytes: 0,
            exit_code: Some(1),
            restart_count: 5,
            last_logs: Some("Traceback ...\nRuntimeError: boom".into()),
        }],
        stats: HostStats::default(),
    };
    ws.send(tungstenite::Message::Text(
        serde_json::to_string(&hb).unwrap().into(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let failures = state.last_failures.read().await;
    let f = failures.get("crashy").expect("crash should be recorded");
    assert_eq!(f.reason, "CrashLoopBackOff", "5 restarts → crashloop");
    assert_eq!(f.exit_code, Some(1));
    assert_eq!(f.restart_count, 5);
    assert!(
        f.message.contains("RuntimeError: boom"),
        "log tail captured"
    );

    ws.close(None).await.ok();
}
