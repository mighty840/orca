//! E2E test: GitHub push webhook validates HMAC and triggers a redeploy.
//!
//! Deploy a service, register a webhook for its repo+branch with an HMAC
//! secret, POST a properly-signed github-style payload, and assert:
//! 1. the request gets 200
//! 2. the invocation is recorded in `state.webhook_invocations` with
//!    `status_code = 200` and `deployed = true`
//! 3. an unsigned re-send is rejected
//!
//! Regression coverage for `webhook::handle_push` and the HMAC validation
//! path. A change that broke matching, signature verification, or invocation
//! recording would surface here.
//!
//! Run with: `cargo test -p orca-control --test e2e_webhook_test -- --ignored`

mod e2e_helpers;

use std::time::Duration;

use e2e_helpers::{TestClient, cleanup_containers, start_server};
use hmac::{Hmac, Mac};
use sha2::Sha256;

const SVC: &str = "e2e-webhook-svc";
const REPO: &str = "e2e/webhook-repo";
const BRANCH: &str = "main";
const SECRET: &str = "shhh-webhook-secret";

fn sign(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[tokio::test]
#[ignore]
async fn e2e_webhook_validates_signature_and_records_invocation() {
    // Isolate the webhook store from `~/.orca/webhooks.json` so we don't read
    // or write the developer's real webhook config. ORCA_WEBHOOKS_PATH is
    // honored by `webhook::webhooks_path`.
    //
    // Safety: this test file has a single `#[tokio::test]`, so no other
    // thread in this binary observes the env mutation.
    let webhook_tmp = tempfile::NamedTempFile::new().unwrap();
    unsafe { std::env::set_var("ORCA_WEBHOOKS_PATH", webhook_tmp.path()) };

    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    // 1. Deploy the service the webhook will target.
    let deploy = serde_json::json!({
        "services": [{
            "name": SVC,
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });
    assert_eq!(
        client.post_json("/api/v1/deploy", &deploy).await.status(),
        200
    );
    tokio::time::sleep(Duration::from_secs(3)).await;

    // 2. Register a webhook for this service with an HMAC secret.
    let resp = client
        .post_json(
            "/api/v1/webhooks",
            &serde_json::json!({
                "repo": REPO,
                "branch": BRANCH,
                "service_name": SVC,
                "secret": SECRET,
                "infra": false
            }),
        )
        .await;
    assert_eq!(resp.status(), 201, "webhook register failed");

    // 3a. Unsigned push must be rejected with 401.
    let body = serde_json::json!({
        "ref": format!("refs/heads/{BRANCH}"),
        "repository": { "full_name": REPO },
        "head_commit": {
            "id": "deadbeefcafebabe",
            "message": "e2e push"
        }
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let resp = client
        .client
        .post(format!("http://127.0.0.1:{port}/api/v1/webhooks/github"))
        .header("Content-Type", "application/json")
        .body(body_bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "unsigned push must be rejected when HMAC secret is configured"
    );

    // 3b. Properly-signed push must succeed.
    let sig = sign(&body_bytes);
    let resp = client
        .client
        .post(format!("http://127.0.0.1:{port}/api/v1/webhooks/github"))
        .header("Content-Type", "application/json")
        .header("X-Hub-Signature-256", sig)
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "signed push should succeed");
    let resp_body: serde_json::Value = resp.json().await.unwrap();
    let deployed = resp_body["deployed"].as_array().unwrap();
    assert!(
        deployed.iter().any(|v| v.as_str() == Some(SVC)),
        "response.deployed should include {SVC}, got {resp_body}"
    );

    // 4. Invocation is recorded in-memory on AppState.
    let invocations = state.webhook_invocations.read().await;
    let ring = invocations
        .get(SVC)
        .unwrap_or_else(|| panic!("no invocation ring for {SVC}"));
    // Both the unsigned (401) and the signed (200) attempt should be present.
    assert!(
        ring.iter().any(|i| i.status_code == 200 && i.deployed),
        "expected a 200/deployed invocation, got {ring:?}"
    );
    assert!(
        ring.iter().any(|i| i.status_code == 401 && !i.deployed),
        "expected a 401 signature-failed invocation, got {ring:?}"
    );
    drop(invocations);

    // Cleanup.
    drop(state);
    cleanup_containers("orca-e2e-").await;
    unsafe { std::env::remove_var("ORCA_WEBHOOKS_PATH") };
}
