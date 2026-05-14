//! E2E test: `orca logs <svc>` returns container output.
//!
//! Deploy nginx, run `orca logs` against the service, assert the subprocess
//! succeeded and produced some output (nginx writes a startup banner to
//! stderr on boot which Docker captures, so the response is non-empty).

use serde_json::json;

use crate::harness::{OrcaServer, require_e2e_env};

#[tokio::test]
#[ignore]
async fn orca_logs_returns_container_output() {
    require_e2e_env();
    let server = OrcaServer::start().await;
    let client = server.client();

    let deploy = json!({
        "services": [{
            "name": "e2e-logs",
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
    // Give nginx a moment to start and emit its startup lines.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let out = server.run_cli(&["logs", "e2e-logs", "--tail", "50"]).await;
    assert!(
        out.status.success(),
        "orca logs exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // nginx:alpine prints a banner on startup. The exact text varies by
    // version so we only assert the response is non-empty — empty stdout
    // would indicate the CLI failed to wire the request through.
    assert!(
        !stdout.trim().is_empty(),
        "expected non-empty log output from nginx, got stdout=\"{stdout}\""
    );
}
