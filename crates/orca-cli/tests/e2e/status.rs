//! E2E test: `orca status` lists running services.
//!
//! Deploy a service, run `orca status`, assert the service name shows up
//! in stdout. Covers the CLI → `/api/v1/status` round-trip end-to-end.

use serde_json::json;

use crate::harness::{OrcaServer, require_e2e_env};

#[tokio::test]
#[ignore]
async fn orca_status_lists_deployed_service() {
    require_e2e_env();
    let server = OrcaServer::start().await;
    let client = server.client();

    let deploy = json!({
        "services": [{
            "name": "e2e-status",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });
    let resp = client
        .post(format!("{}/api/v1/deploy", server.api_url))
        .json(&deploy)
        .send()
        .await
        .expect("deploy");
    assert!(resp.status().is_success() || resp.status().as_u16() == 206);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let out = server.run_cli(&["status"]).await;
    assert!(
        out.status.success(),
        "orca status exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("e2e-status"),
        "expected service name in `orca status` output, got:\n{stdout}"
    );
}
