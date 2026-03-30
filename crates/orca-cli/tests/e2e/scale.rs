//! E2E test: deploy a service, then scale it up and down.

use serde_json::json;

use crate::harness::{OrcaServer, require_e2e_env};

/// Deploy a service with 1 replica, scale up to 3, then back down to 1.
///
/// Run with: `ORCA_E2E=1 cargo test -p orca-cli --test main -- --ignored scale`
#[tokio::test]
#[ignore]
async fn scale_service_up_and_down() {
    require_e2e_env();

    let server = OrcaServer::start().await;
    let client = server.client();

    // Deploy nginx:alpine with 1 replica.
    let deploy_body = json!({
        "services": [{
            "name": "e2e-scale",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });

    let resp = client
        .post(format!("{}/api/v1/deploy", server.api_url))
        .json(&deploy_body)
        .send()
        .await
        .expect("deploy request failed");

    assert!(
        resp.status().is_success() || resp.status().as_u16() == 206,
        "deploy failed: {}",
        resp.status()
    );

    // Wait for the container to start.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Scale up to 3 replicas.
    let scale_up = json!({ "replicas": 3 });
    let resp = client
        .post(format!(
            "{}/api/v1/services/e2e-scale/scale",
            server.api_url
        ))
        .json(&scale_up)
        .send()
        .await
        .expect("scale-up request failed");

    assert!(
        resp.status().is_success(),
        "scale-up failed: {}",
        resp.status()
    );

    let scale_resp: serde_json::Value = resp.json().await.expect("failed to parse scale response");
    assert_eq!(
        scale_resp["replicas"].as_u64().unwrap_or(0),
        3,
        "scale response should show 3 replicas: {scale_resp}"
    );

    // Wait for scaling to take effect.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Verify via status.
    let status: serde_json::Value = client
        .get(format!("{}/api/v1/status", server.api_url))
        .send()
        .await
        .expect("status request failed")
        .json()
        .await
        .expect("failed to parse status");

    let svc = status["services"]
        .as_array()
        .expect("missing services")
        .iter()
        .find(|s| s["name"].as_str() == Some("e2e-scale"))
        .expect("e2e-scale not in status");

    assert_eq!(
        svc["desired_replicas"].as_u64().unwrap_or(0),
        3,
        "expected 3 desired replicas: {svc}"
    );

    // Scale back down to 1.
    let scale_down = json!({ "replicas": 1 });
    let resp = client
        .post(format!(
            "{}/api/v1/services/e2e-scale/scale",
            server.api_url
        ))
        .json(&scale_down)
        .send()
        .await
        .expect("scale-down request failed");

    assert!(
        resp.status().is_success(),
        "scale-down failed: {}",
        resp.status()
    );

    let scale_resp: serde_json::Value = resp.json().await.expect("failed to parse scale response");
    assert_eq!(
        scale_resp["replicas"].as_u64().unwrap_or(0),
        1,
        "scale response should show 1 replica: {scale_resp}"
    );

    // Wait and verify.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let status: serde_json::Value = client
        .get(format!("{}/api/v1/status", server.api_url))
        .send()
        .await
        .expect("status request failed")
        .json()
        .await
        .expect("failed to parse status");

    let svc = status["services"]
        .as_array()
        .expect("missing services")
        .iter()
        .find(|s| s["name"].as_str() == Some("e2e-scale"))
        .expect("e2e-scale not in status");

    assert_eq!(
        svc["desired_replicas"].as_u64().unwrap_or(0),
        1,
        "expected 1 desired replica after scale-down: {svc}"
    );
}
