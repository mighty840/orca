//! E2E test: cluster networks dashboard reflects deployed services.
//!
//! Deploy a service on a named network, hit `GET /api/v1/cluster/networks`,
//! and assert the master node entry includes the expected `orca-*` bridge
//! with the service container listed.
//!
//! Regression coverage for the #17 endpoint (`api/handlers/ops/networks.rs`)
//! and `enumerate_orca_networks` in `orca-agent`. A change that broke the
//! per-network container enumeration (e.g. dropping the container-centric
//! lookup that surfaces aliases) would fail this test.
//!
//! Run with: `cargo test -p orca-control --test e2e_cluster_networks_test -- --ignored`

mod e2e_helpers;

use std::time::Duration;

use e2e_helpers::{TestClient, cleanup_containers, start_server};
use orca_core::api_types::ClusterNetworksResponse;

#[tokio::test]
#[ignore]
async fn e2e_cluster_networks_lists_deployed_service() {
    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    // Deploy two services on the same named network — verifies the endpoint
    // groups multiple containers under one bridge.
    let deploy = serde_json::json!({
        "services": [
            {
                "name": "e2e-netdash-a",
                "image": "nginx:alpine",
                "replicas": 1,
                "port": 80,
                "network": "netdash"
            },
            {
                "name": "e2e-netdash-b",
                "image": "nginx:alpine",
                "replicas": 1,
                "port": 80,
                "network": "netdash"
            }
        ]
    });
    let resp = client.post_json("/api/v1/deploy", &deploy).await;
    assert_eq!(resp.status(), 200, "deploy failed: {}", resp.status());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let resp = client.get("/api/v1/cluster/networks").await;
    assert_eq!(
        resp.status(),
        200,
        "cluster_networks failed: {}",
        resp.status()
    );
    let body: ClusterNetworksResponse = resp.json().await.unwrap();

    // Find the master entry. In this test there are no joined agents, so
    // there should be exactly one node row with `node_id == None`.
    let master = body
        .nodes
        .iter()
        .find(|n| n.node_id.is_none())
        .expect("master row missing from /api/v1/cluster/networks");
    assert!(master.reachable, "master should always be reachable");

    let bridge = master
        .networks
        .iter()
        .find(|n| n.name == "orca-netdash")
        .unwrap_or_else(|| {
            panic!(
                "orca-netdash bridge missing from master networks (found: {:?})",
                master.networks.iter().map(|n| &n.name).collect::<Vec<_>>()
            )
        });

    let names: Vec<&str> = bridge.services.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("e2e-netdash-a")),
        "e2e-netdash-a should appear in the bridge's service list, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("e2e-netdash-b")),
        "e2e-netdash-b should appear in the bridge's service list, got {names:?}"
    );

    drop(state);
    cleanup_containers("orca-e2e-").await;
}
