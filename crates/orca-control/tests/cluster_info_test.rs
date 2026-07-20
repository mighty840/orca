//! Regression test for the `orca nodes` / `orca nodes --gpus` decode error.
//!
//! `handle_nodes` used a raw unauthenticated client and decoded the response
//! as JSON without checking status, so against a token-protected master it
//! got a 401 plain-text body and failed with "expected value at line 1
//! column 1". This verifies (a) cluster/info requires auth (the 401 the CLI
//! must now send a token for and surface cleanly) and (b) the response
//! carries declared GPUs so `--gpus` has data.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::{AppState, RegisteredNode};
use orca_core::config::{ClusterConfig, ClusterMeta, NodeConfig, NodeGpuConfig};

const TOKEN: &str = "cluster-info-token";

async fn start() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "gpu-cluster".into(),
            api_port: port,
            ..Default::default()
        },
        api_tokens: vec![TOKEN.into()],
        // Declared GPUs live in cluster.toml, matched to registered nodes by
        // address (host portion): declared "gpu-box" ~ registered "gpu-box:6881".
        node: vec![NodeConfig {
            address: "gpu-box".into(),
            labels: HashMap::new(),
            gpus: vec![NodeGpuConfig {
                vendor: "nvidia".into(),
                count: 2,
                model: Some("A100".into()),
            }],
        }],
        ..Default::default()
    };
    let state = Arc::new(AppState::new(
        config,
        Arc::new(orca_core::testing::MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));
    state.registered_nodes.write().await.insert(
        7,
        RegisteredNode {
            peer_ip: None,
            node_id: 7,
            address: "gpu-box:6881".into(),
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
    let app = orca_control::api::router(state);
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    port
}

#[tokio::test]
async fn cluster_info_requires_auth_and_surfaces_gpus() {
    let port = start().await;

    // Unauthenticated: the exact failure `orca nodes` hit — 401, non-JSON body.
    let no_auth = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/v1/cluster/info"))
        .send()
        .await
        .unwrap();
    assert_eq!(no_auth.status(), 401, "cluster/info must require a token");

    // Authenticated: 200 with declared GPUs matched onto the registered node.
    let ok = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/v1/cluster/info"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let json: serde_json::Value = ok.json().await.unwrap();
    let node = &json["nodes"][0];
    assert_eq!(node["node_id"], 7);
    assert_eq!(node["gpus"][0]["model"], "A100");
    assert_eq!(node["gpus"][0]["count"], 2);
}
