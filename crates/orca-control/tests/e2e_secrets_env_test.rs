//! E2E test: `${secrets.KEY}` interpolation at deploy time.
//!
//! Set a secret via the API, deploy a service whose env references it, then
//! inspect the resulting container and assert the environment variable was
//! resolved to the secret's plaintext value before the container started.
//!
//! Regression coverage for `resolve_secrets` in `routes.rs`: a change that
//! silently dropped or mishandled the resolution would leave the literal
//! `${secrets.X}` in the container env, which this test catches.
//!
//! NOTE: this test reads from and writes to `~/.orca/secrets.json` because
//! `SecretStore::open(default_path())` is hardcoded to the user's home
//! directory. We use a unique key prefix to avoid collisions with real
//! secrets the developer may have, and delete the key after the run.
//!
//! Run with: `cargo test -p orca-control --test e2e_secrets_env_test -- --ignored`

mod e2e_helpers;

use std::time::Duration;

use e2e_helpers::{TestClient, cleanup_containers, start_server};

const SECRET_KEY: &str = "E2E_SECRET_INTERPOLATE_XYZ";
const SECRET_VALUE: &str = "interpolated-value-12345";

#[tokio::test]
#[ignore]
async fn e2e_secret_value_resolved_in_container_env() {
    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    // Set a known secret via the API. This writes to ~/.orca/secrets.json.
    let resp = client
        .post_json(
            &format!("/api/v1/secrets/{SECRET_KEY}"),
            &serde_json::json!({ "value": SECRET_VALUE }),
        )
        .await;
    assert!(
        resp.status().is_success(),
        "set_secret failed: {}",
        resp.status()
    );

    // Deploy a service whose env references the secret. The container itself
    // doesn't need to do anything — we only assert what Docker sees in its
    // environment block via `inspect_container`.
    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-secret-env",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "env": {
                "TEST_FROM_SECRET": format!("${{secrets.{SECRET_KEY}}}"),
                "TEST_LITERAL": "not-a-secret"
            }
        }]
    });
    let resp = client.post_json("/api/v1/deploy", &deploy).await;
    assert_eq!(resp.status(), 200, "deploy failed: {}", resp.status());

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Inspect the container directly and parse its env array.
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let info = docker
        .inspect_container("orca-e2e-secret-env", None)
        .await
        .expect("container should exist after deploy");
    let env_pairs: Vec<String> = info.config.and_then(|c| c.env).unwrap_or_default();

    let test_from_secret = env_pairs
        .iter()
        .find_map(|p| p.strip_prefix("TEST_FROM_SECRET="))
        .expect("TEST_FROM_SECRET must be in container env");
    assert_eq!(
        test_from_secret, SECRET_VALUE,
        "`${{secrets.{SECRET_KEY}}}` should have been replaced with the secret value at deploy time, \
         got `{test_from_secret}` in container env"
    );

    let test_literal = env_pairs
        .iter()
        .find_map(|p| p.strip_prefix("TEST_LITERAL="))
        .expect("TEST_LITERAL must be in container env");
    assert_eq!(
        test_literal, "not-a-secret",
        "non-templated env vars must pass through unchanged"
    );

    // Cleanup: drop the secret and the deployed service.
    let _ = client
        .client
        .delete(format!(
            "http://127.0.0.1:{port}/api/v1/secrets/{SECRET_KEY}"
        ))
        .send()
        .await;
    drop(state);
    cleanup_containers("orca-e2e-").await;
}
