//! E2E test: `orca stop <svc>` removes the running container.
//!
//! Deploy nginx, verify the container exists via Docker inspect, run
//! `orca stop`, assert the container is gone.

use serde_json::json;

use crate::harness::{OrcaServer, require_e2e_env};

#[tokio::test]
#[ignore]
async fn orca_stop_removes_container() {
    require_e2e_env();
    let server = OrcaServer::start().await;
    let client = server.client();

    let deploy = json!({
        "services": [{
            "name": "e2e-stop",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });
    client
        .post(format!("{}/api/v1/deploy", server.api_url))
        .json(&deploy)
        .send()
        .await
        .expect("deploy");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    docker
        .inspect_container("orca-e2e-stop", None)
        .await
        .expect("container should exist after deploy");

    let out = server.run_cli(&["stop", "e2e-stop"]).await;
    assert!(
        out.status.success(),
        "orca stop exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    assert!(
        docker
            .inspect_container("orca-e2e-stop", None)
            .await
            .is_err(),
        "container should have been removed by `orca stop`"
    );
}
