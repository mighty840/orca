//! E2E test: `orca rollback <svc>` restores the previous image.
//!
//! Deploy v1 (`nginx:1.27-alpine`), wait, deploy v2 (`nginx:1.28-alpine`),
//! wait, run `orca rollback`, assert the container image is back to v1.

use serde_json::json;

use crate::harness::{OrcaServer, require_e2e_env};

#[tokio::test]
#[ignore]
async fn orca_rollback_restores_previous_image() {
    require_e2e_env();
    let server = OrcaServer::start().await;
    let client = server.client();

    let v1_image = "nginx:1.27-alpine";
    let v2_image = "nginx:1.28-alpine";

    // Initial deploy at v1.
    let body_v1 = json!({
        "services": [{
            "name": "e2e-rollback",
            "image": v1_image,
            "replicas": 1,
            "port": 80
        }]
    });
    client
        .post(format!("{}/api/v1/deploy", server.api_url))
        .json(&body_v1)
        .send()
        .await
        .expect("deploy v1");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Upgrade to v2 (different image).
    let body_v2 = json!({
        "services": [{
            "name": "e2e-rollback",
            "image": v2_image,
            "replicas": 1,
            "port": 80
        }]
    });
    client
        .post(format!("{}/api/v1/deploy", server.api_url))
        .json(&body_v2)
        .send()
        .await
        .expect("deploy v2");
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;

    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let after_v2 = docker
        .inspect_container("orca-e2e-rollback", None)
        .await
        .expect("v2 container should exist");
    let v2_running = after_v2.config.and_then(|c| c.image).unwrap_or_default();
    assert!(
        v2_running.contains("1.28"),
        "v2 deploy should be on the 1.28 image, got {v2_running:?}"
    );

    // Roll back.
    let out = server.run_cli(&["rollback", "e2e-rollback"]).await;
    assert!(
        out.status.success(),
        "orca rollback exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let rolled_back = docker
        .inspect_container("orca-e2e-rollback", None)
        .await
        .expect("container should exist after rollback");
    let img = rolled_back.config.and_then(|c| c.image).unwrap_or_default();
    assert!(
        img.contains("1.27"),
        "rollback should restore the v1 image, got {img:?}"
    );
}
