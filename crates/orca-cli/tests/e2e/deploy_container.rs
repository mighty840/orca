//! E2E test: deploy a container, check status, and retrieve logs.

use serde_json::json;

use crate::harness::{OrcaServer, require_e2e_env};

/// Deploy nginx:alpine, verify it appears in status, and fetch logs.
///
/// Requires Docker and a built `orca` binary.
/// Run with: `ORCA_E2E=1 cargo test -p orca-cli --test main -- --ignored deploy`
#[tokio::test]
#[ignore]
async fn deploy_container_and_check_status() {
    require_e2e_env();

    let server = OrcaServer::start().await;
    let client = server.client();

    // Deploy nginx:alpine via the API.
    let deploy_body = json!({
        "services": [{
            "name": "e2e-nginx",
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

    let status_code = resp.status();
    assert!(
        status_code.is_success() || status_code.as_u16() == 206,
        "deploy returned unexpected status: {status_code}"
    );

    let deploy_resp: serde_json::Value =
        resp.json().await.expect("failed to parse deploy response");
    let deployed = deploy_resp["deployed"]
        .as_array()
        .expect("missing deployed array");
    assert!(
        deployed.iter().any(|v| v.as_str() == Some("e2e-nginx")),
        "e2e-nginx not in deployed list: {deploy_resp}"
    );

    // Wait for container to start.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Check status endpoint.
    let status_resp: serde_json::Value = client
        .get(format!("{}/api/v1/status", server.api_url))
        .send()
        .await
        .expect("status request failed")
        .json()
        .await
        .expect("failed to parse status");

    let services = status_resp["services"]
        .as_array()
        .expect("missing services array");
    let nginx_svc = services
        .iter()
        .find(|s| s["name"].as_str() == Some("e2e-nginx"))
        .expect("e2e-nginx not found in status");

    assert_eq!(
        nginx_svc["running_replicas"].as_u64().unwrap_or(0),
        1,
        "expected 1 running replica, got: {nginx_svc}"
    );

    // Fetch logs (may be empty if nginx hasn't served any requests yet).
    let logs_resp = client
        .get(format!(
            "{}/api/v1/services/e2e-nginx/logs?tail=10&follow=false",
            server.api_url
        ))
        .send()
        .await
        .expect("logs request failed");

    assert!(
        logs_resp.status().is_success(),
        "logs request returned: {}",
        logs_resp.status()
    );

    // The logs endpoint returns text — just verify we got a response.
    let _logs_text = logs_resp.text().await.expect("failed to read logs body");
}
