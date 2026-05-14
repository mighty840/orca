//! E2E test: `orca redeploy <svc>` replaces the running container.
//!
//! Deploy nginx, capture the resulting container ID, run `orca redeploy`,
//! assert a new container ID exists (i.e. the old container was replaced).

use serde_json::json;

use crate::harness::{OrcaServer, require_e2e_env};

#[tokio::test]
#[ignore]
async fn orca_redeploy_replaces_container() {
    require_e2e_env();
    let server = OrcaServer::start().await;
    let client = server.client();

    let deploy = json!({
        "services": [{
            "name": "e2e-redeploy",
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
    let before = docker
        .inspect_container("orca-e2e-redeploy", None)
        .await
        .expect("container should exist after deploy");
    let before_id = before.id.expect("container should have an id");

    let out = server.run_cli(&["redeploy", "e2e-redeploy"]).await;
    assert!(
        out.status.success(),
        "orca redeploy exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let after = docker
        .inspect_container("orca-e2e-redeploy", None)
        .await
        .expect("container should exist after redeploy");
    let after_id = after.id.expect("container should have an id");
    assert_ne!(
        before_id, after_id,
        "redeploy should create a new container; got the same id={before_id}"
    );
    assert!(
        after.state.and_then(|s| s.running).unwrap_or(false),
        "new container should be running after redeploy"
    );
}
